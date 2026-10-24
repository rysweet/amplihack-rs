#!/usr/bin/env bash
# test-issue-1626-consensus-rerun-preserves-work.sh — TDD spec for issue #1626.
#
# Issue #1626: consensus-issue-worktree.yaml step3-setup-worktree, on a re-run
# whose branch already exists, destroyed work. State 1 (branch + worktree,
# dirty or ahead of $BASE_REF) ran `checkout -- .`, `clean -fd` and
# `reset --hard "$BASE_REF"`; State 2 (branch, no worktree, ahead) ran
# `git branch -D` and recreated the branch from the base.
#
# REQUIRED BEHAVIOUR (same contract as workflow-worktree.yaml step-04, #1565):
#   - Work is never reset or deleted. A dirty or ahead branch is KEPT; the run
#     moves to a fresh `<branch>-<suffix>` branch chosen by
#     workflow_worktree_deconflict.sh `resolve --base "$BASE_REF"`.
#   - The helper's answer is untrusted: it must pass check-ref-format, use only
#     [A-Za-z0-9._/-], and equal the original name or `<original>-*`.
#   - If work still turns up after deconfliction, or the helper is missing and
#     there is work, the step refuses (ERROR, non-zero) instead of resetting.
#   - A clean existing branch is reused unchanged.
#   - The guard runs on the RESOLVED name: if the helper's suffixed answer
#     itself holds work, the step refuses and nothing is pushed (S13, both
#     states). The push follows the resolved branch, never the original (S1/S2).
#   - The helper is located the way step-04 does it: the 5-place ladder
#     (AMPLIHACK_HOME, else REPO_PATH, else $(pwd); REPO_PATH; $(pwd);
#     ~/.copilot; ~/.amplihack), then TOOLS_DIR and step-04's two
#     DECONFLICT_HELPER lines, gated by `if [ -f "$DECONFLICT_HELPER" ]` (A8).
#     With AMPLIHACK_HOME empty, a helper in the repo is used, and its answer
#     is still validated (S11b).
#   - The slug path prints exactly one JSON line on stdout that carries the
#     resolved branch_name and worktree_path.
#
# Part A: static checks on the step3 fenced bash block.
# Part S: the real step3 bash, extracted from the YAML, run against scratch
#         repos with a local bare origin.
#
# Override: WORKTREE_YAML=<regular file> runs Part A/S against another copy of
# the recipe, e.g. origin/main's, to show the test fails there:
#   git show origin/main:amplifier-bundle/recipes/consensus-issue-worktree.yaml \
#     > "$TMPDIR/i1626-main.yaml"
#   WORKTREE_YAML="$TMPDIR/i1626-main.yaml" bash <this script>   # must exit 1
#
# Nothing here touches the network or /tmp: scratch lives under $TMPDIR,
# origin is a bare repo inside it, and GIT_ALLOW_PROTOCOL=file.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1626-consensus-rerun-preserves-work.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
WORKTREE_YAML="${WORKTREE_YAML:-${REPO_ROOT}/amplifier-bundle/recipes/consensus-issue-worktree.yaml}"
HELPER="${REPO_ROOT}/amplifier-bundle/tools/workflow_worktree_deconflict.sh"
HELPER_REL="amplifier-bundle/tools/workflow_worktree_deconflict.sh"

# The step derives feat/consensus-<slug> from TASK_DESCRIPTION. S0 checks the
# emitted name equals ORIG, so the test never re-implements the slug rules.
STEP_TASK_DESC="fix issue 1626"
ORIG="feat/consensus-fix-issue-1626"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }
harness() { echo "HARNESS-ERROR: $*" >&2; exit 2; }

[[ -f "${WORKTREE_YAML}" && ! -L "${WORKTREE_YAML}" ]] \
    || harness "WORKTREE_YAML must be a regular file: ${WORKTREE_YAML}"
[[ -f "${HELPER}" ]] || harness "deconflict helper not found: ${HELPER}"
command -v python3 >/dev/null 2>&1 || harness "python3 is required"

# --- Hermetic scratch (never /tmp). ------------------------------------------
: "${TMPDIR:?TMPDIR must be set (scratch must not land on /tmp)}"
case "${TMPDIR%/}" in
    /tmp|/tmp/*) harness "TMPDIR points under /tmp; use a disk-backed TMPDIR" ;;
esac
SCRATCH="$(mktemp -d "${TMPDIR%/}/i1626.XXXXXX")" || harness "mktemp failed"
cleanup() {
    case "${SCRATCH:-}" in
        "${TMPDIR%/}"/i1626.?*) rm -rf -- "${SCRATCH}" ;;
    esac
}
trap cleanup EXIT
mkdir -p "${SCRATCH}/home" "${SCRATCH}/tmp"

export HOME="${SCRATCH}/home"
# Isolation pre-flight: step-04's ladder also looks in ~/.copilot and
# ~/.amplihack. Neither may exist, or a real helper could answer for the test.
for _d in "${HOME}/.amplihack" "${HOME}/.copilot"; do
    [[ ! -e "${_d}" ]] || harness "scratch HOME must not contain ${_d##*/}"
done
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_TERMINAL_PROMPT=0 GIT_ALLOW_PROTOCOL=file
export GIT_AUTHOR_NAME="Test" GIT_AUTHOR_EMAIL="t@example.com"
export GIT_COMMITTER_NAME="Test" GIT_COMMITTER_EMAIL="t@example.com"
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

# --- AMPLIHACK_HOME variants. HOME/.amplihack never exists. -------------------
AHOME_OK="${SCRATCH}/ahome-ok"
AHOME_NONE="${SCRATCH}/ahome-none"
mkdir -p "${AHOME_OK}/amplifier-bundle/tools" "${AHOME_NONE}"
cp -- "${HELPER}" "${AHOME_OK}/${HELPER_REL}"

# make_stub <name> <body> -> AMPLIHACK_HOME path whose helper runs <body>.
make_stub() {
    local d="${SCRATCH}/ahome-stub-$1"
    mkdir -p "${d}/amplifier-bundle/tools"
    printf '#!/usr/bin/env bash\n%s\n' "$2" > "${d}/${HELPER_REL}"
    printf '%s\n' "${d}"
}

# ---------------------------------------------------------------------------
# Extraction: the first ```bash fence after `- id: "step3-setup-worktree"`,
# with the 6-space prompt indent stripped, up to the closing fence.
# ---------------------------------------------------------------------------
extract_step3_bash() {
    awk '
        /^[[:space:]]*- id: "step3-setup-worktree"[[:space:]]*$/ { found = 1; next }
        found && !inblk && /^[[:space:]]*- id: "/ { exit }
        found && !inblk && $0 == "      ```bash" { inblk = 1; next }
        inblk && /^      ```[[:space:]]*$/ { done = 1; exit }
        inblk { line = $0; sub(/^      /, "", line); print line }
        END { if (!done) exit 1 }
    ' "$1"
}

STEP_SCRIPT="${SCRATCH}/step3-body.sh"
extract_step3_bash "${WORKTREE_YAML}" > "${STEP_SCRIPT}" \
    || harness "could not extract a closed \`\`\`bash block from step3-setup-worktree"
[[ -s "${STEP_SCRIPT}" ]] || harness "extracted step3 block is empty"
STEP3="$(cat "${STEP_SCRIPT}")"

# build_repo <name> -> clone of a fresh local bare origin (main + origin/HEAD).
build_repo() {
    local name="$1"
    local remote="${SCRATCH}/${name}-origin.git" work="${SCRATCH}/${name}"
    git init --quiet --bare "${remote}"
    git clone --quiet "${remote}" "${work}" 2>/dev/null
    git -C "${work}" checkout -q -b main 2>/dev/null || git -C "${work}" checkout -q main
    echo "base" > "${work}/README.md"
    git -C "${work}" add README.md
    git -C "${work}" commit -q -m "base commit"
    git -C "${work}" push -q -u origin main 2>/dev/null
    git -C "${work}" remote set-head origin main >/dev/null 2>&1
    local url
    url="$(git -C "${work}" remote get-url origin)"
    case "${url}" in
        "${SCRATCH}"/*) ;;
        *) harness "origin '${url}' is outside scratch" ;;
    esac
    printf '%s\n' "${work}"
}

# Each repo's worktrees live in <repo>-wts (absolute WORKTREE_DIR).
wtdir() { printf '%s-wts\n' "$1"; }

# state1 <repo> -> ORIG checked out at <wtdir>/ORIG, based on origin/main.
state1() { git -C "$1" worktree add -q -b "${ORIG}" -- "$(wtdir "$1")/${ORIG}" origin/main; }

# commit_ahead <worktree> -> one unpushed commit (ahead.txt).
commit_ahead() {
    echo "unpushed work" > "$1/ahead.txt"
    git -C "$1" add ahead.txt
    git -C "$1" commit -q -m "unpushed work"
}

# make_dirty <worktree> -> a modified tracked file and an untracked file.
make_dirty() {
    echo "modified tracked content" > "$1/README.md"
    echo "untracked content" > "$1/untracked.txt"
}

# run_step <repo> [amplihack_home] [worktree_dir] -> RUN_RC, RUN_STDOUT,
# RUN_STDERR, RUN_JSON. worktree_dir defaults to <repo>-wts.
# amplihack_home defaults to AHOME_OK only when OMITTED: an explicit "" is
# passed through as an empty AMPLIHACK_HOME (S11b needs that).
run_step() {
    local repo="$1" ahome="${2-${AHOME_OK}}" wtd="${3:-$(wtdir "$1")}"
    local out="${SCRATCH}/run.out" err="${SCRATCH}/run.err"
    RUN_RC=0
    (
        cd "${repo}" && env -i \
            PATH="${PATH}" \
            HOME="${HOME}" \
            TMPDIR="${SCRATCH}/tmp" \
            GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1 \
            GIT_TERMINAL_PROMPT=0 GIT_ALLOW_PROTOCOL=file \
            GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=t@example.com \
            GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=t@example.com \
            AMPLIHACK_HOME="${ahome}" \
            REPO_PATH="${repo}" \
            WORKTREE_DIR="${wtd}" \
            TASK_DESCRIPTION="${STEP_TASK_DESC}" \
            bash "${STEP_SCRIPT}"
    ) >"${out}" 2>"${err}" </dev/null || RUN_RC=$?
    RUN_STDOUT="$(cat "${out}")"
    RUN_STDERR="$(cat "${err}")"
    RUN_JSON="$(awk '/^\{/{l=$0} END{print l}' <<<"${RUN_STDOUT}")"
}

# json_field <key> -> value from RUN_JSON ("" if absent or unparseable).
json_field() {
    printf '%s' "${RUN_JSON}" | python3 -I -c '
import json, sys
try:
    v = json.load(sys.stdin).get(sys.argv[1], "")
except Exception:
    v = ""
if isinstance(v, bool):
    v = str(v).lower()
elif isinstance(v, list):
    v = ",".join(str(x) for x in v)
print(v)
' "$1"
}

last_lines() {
    awk -v n="$1" '{ring[NR % n] = $0} END {for (i = NR - n + 1; i <= NR; i++) if (i > 0) print ring[i % n]}' <<<"$2"
}

# file_sum <path> -> sha256 of the file ("missing" if absent).
file_sum() { if [[ -f "$1" ]]; then sha256sum -- "$1" | awk '{print $1}'; else echo missing; fi; }

# heads <repo> -> every local branch and its SHA.
heads() { git -C "$1" for-each-ref --format='%(refname) %(objectname)' refs/heads; }

# wt_list <repo> -> registered worktree paths, one per line.
wt_list() { git -C "$1" worktree list --porcelain | awk '$1=="worktree"{print substr($0,10)}'; }

wt_registered() { wt_list "$1" | grep -qFx -- "$2"; }

# ===========================================================================
# Part A — static checks on the step3 fenced bash block.
# ===========================================================================
echo "=== Part A: step3 static checks (${WORKTREE_YAML}) ==="

# A0: the extracted block is valid bash.
if bash -n "${STEP_SCRIPT}" 2>"${SCRATCH}/bash-n.err"; then
    pass "A0-bash-syntax" "extracted step3 block parses with bash -n"
else
    fail "A0-bash-syntax" "bash -n rejects step3: $(last_lines 3 "$(cat "${SCRATCH}/bash-n.err")")"
fi

# destructive_hits <text> -> every line running a command that resets, deletes
# or force-moves work. Lines that only print text (echo/printf) are skipped so
# a message such as "run 'git worktree prune'" in the #342 path is not flagged.
# A compound line such as "echo x; git reset --hard" would also be skipped;
# nothing in step3 has that shape, and the trade-off avoids flagging the #342
# prune hint.
# Comments are scanned too: the step must not even describe these as options.
destructive_hits() {
    local G='git([[:space:]]+(-C|-c)[[:space:]]+[^[:space:]]+|[[:space:]]+--no-optional-locks)*[[:space:]]+'
    local E='([[:space:]]|$|;|\)|&|\|)'
    local re=""
    re+="${G}checkout([[:space:]]+-[^[:space:]]*)*[[:space:]]+--[[:space:]]+\\.${E}"
    re+="|${G}clean([[:space:]]+-[^[:space:]]*)*[[:space:]]+(-[a-zA-Z]*f[a-zA-Z]*|--force)${E}"
    re+="|${G}reset([[:space:]]+[^[:space:]]+)*[[:space:]]+--hard${E}"
    re+="|${G}branch([[:space:]]+-[^[:space:]]*)*[[:space:]]+(-[a-zA-Z]*[Df][a-zA-Z]*|--force)${E}"
    re+="|${G}worktree[[:space:]]+add([[:space:]]+[^[:space:]]+)*[[:space:]]+(-f|--force|-B)${E}"
    re+="|${G}worktree[[:space:]]+(remove|prune)${E}"
    re+="|${G}update-ref${E}"
    re+="|${G}push([[:space:]]+[^[:space:]]+)*[[:space:]]+(-[a-zA-Z]*f[a-zA-Z]*|--force|--force-with-lease(=[^[:space:]]*)?|--force-if-includes|\"?\\+[^[:space:]]+)${E}"
    re+="|(^|[[:space:];&|(])rm([[:space:]]+-[^[:space:]]*)*[[:space:]]+(-[a-zA-Z]*[rR][a-zA-Z]*|--recursive)${E}"
    printf '%s\n' "$1" \
        | awk '{ t = $0; sub(/^[[:space:]]+/, "", t); if (t !~ /^(echo|printf)[[:space:]]/) print }' \
        | grep -nE -- "${re}" || true
}

# A1-self: the detector must catch every banned form and pass the safe ones.
SELF_BAD=$'git checkout -- .\ngit -C "$W" checkout -- .\ngit clean -fd\ngit clean -fdx\ngit clean -d -f\ngit clean --force\ngit -C "$W" reset --hard "$BASE_REF"\ngit reset -q --hard\ngit branch -D x\ngit branch -f x y\ngit branch --force x\ngit worktree add -B x p base\ngit worktree add --force p x\ngit worktree add -f p x\ngit worktree remove p\ngit worktree prune\ngit update-ref refs/heads/x y\ngit push --force origin x\ngit push -f origin x\ngit push -uf origin x\ngit push --force-with-lease origin x\ngit push origin +x\ngit push origin "+refs/heads/x:refs/heads/x"\nrm -rf -- "$W"\nrm -fr "$W"\nrm -r -f x\n  # git reset --hard in a comment'
SELF_OK=$'git branch --list x\nif git check-ref-format --branch "$RESOLVED_BRANCH" >/dev/null 2>&1; then\ngit push -u origin "$BRANCH_NAME" > /dev/null 2>&1 || \\\ngit fetch origin "+refs/heads/${B}:refs/remotes/origin/${B}" >&2 2>/dev/null || true\ngit worktree add -b "$B" -- "$P" "$BASE_REF" >&2\ngit worktree add -- "$P" "$B" >&2\ngit worktree list --porcelain\ngit show-ref --verify --quiet "refs/heads/$B"\nrm -f -- "$F"\n  echo "ERROR: run \'git worktree prune\' first" >&2\ngit -C "$P" status --porcelain'
SELF_MISSED=""
while IFS= read -r l; do
    [[ -n "$(destructive_hits "$l")" ]] || SELF_MISSED+=" [$l]"
done <<<"${SELF_BAD}"
SELF_FALSE=""
while IFS= read -r l; do
    [[ -z "$(destructive_hits "$l")" ]] || SELF_FALSE+=" [$l]"
done <<<"${SELF_OK}"
if [[ -z "${SELF_MISSED}" && -z "${SELF_FALSE}" ]]; then
    pass "A1-self" "destructive-command detector catches every banned form and no safe form"
else
    harness "A1 detector is wrong: missed:${SELF_MISSED:- none}; false positives:${SELF_FALSE:- none}"
fi

# A1: no destructive command anywhere in the step3 block.
HITS="$(destructive_hits "${STEP3}")"
if [[ -z "${HITS}" ]]; then
    pass "A1-no-destructive" "step3 contains no reset/clean/checkout -- ./branch -D/force/rm -rf"
else
    fail "A1-no-destructive" "step3 still contains destructive commands:"$'\n'"${HITS}"
fi

# A2: the helper is called with resolve --base "$BASE_REF".
if grep -qE 'resolve[[:space:]]+--base[[:space:]]+"\$\{?BASE_REF\}?"' <<<"${STEP3}"; then
    pass "A2-passes-base" "step3 calls the helper with resolve --base \"\$BASE_REF\""
else
    fail "A2-passes-base" "step3 does not call 'resolve --base \"\$BASE_REF\"'"
fi

# A3: the helper's answer is validated with check-ref-format.
if grep -qE 'check-ref-format[[:space:]]+--branch[[:space:]]+"\$\{?RESOLVED_BRANCH\}?"' <<<"${STEP3}"; then
    pass "A3-validates-resolved" "step3 validates RESOLVED_BRANCH with check-ref-format --branch"
else
    fail "A3-validates-resolved" "step3 does not validate RESOLVED_BRANCH with check-ref-format --branch"
fi

# A4: the literal that test-foreign-worktree-deconflict.sh A17 depends on.
if grep -qF 'deconfliction failed' <<<"${STEP3}"; then
    pass "A4-deconfliction-failed" "step3 keeps the 'deconfliction failed' error"
else
    fail "A4-deconfliction-failed" "step3 lost the 'deconfliction failed' error"
fi

# A5: refuse_reset is defined and called from both State 1 and State 2.
RR_CALLS="$(grep -cE '(^|[[:space:];&|{(])refuse_reset([[:space:];]|$)' <<<"${STEP3}" || true)"
if grep -qE 'refuse_reset[[:space:]]*\(\)' <<<"${STEP3}" && [[ "${RR_CALLS:-0}" -ge 2 ]]; then
    pass "A5-refuse-reset" "refuse_reset is defined and called ${RR_CALLS} times"
else
    fail "A5-refuse-reset" "refuse_reset missing or called fewer than 2 times (calls=${RR_CALLS:-0})"
fi

# A6: the slug path prints the JSON line too (the #342 path already prints one).
PRINTF_JSON="$(grep -cF "printf '{\"branch_name\"" <<<"${STEP3}" || true)"
if [[ "${PRINTF_JSON:-0}" -ge 2 ]]; then
    pass "A6-json-printf" "both the #342 path and the slug path print the JSON line"
else
    fail "A6-json-printf" "only ${PRINTF_JSON:-0} printf JSON line(s); the slug path must print its own"
fi

# A7: branch existence uses show-ref --verify, not glob-matching branch --list (SR-7).
if grep -qE 'git[[:space:]]+branch[[:space:]]+--list' <<<"${STEP3}"; then
    fail "A7-no-branch-list" "step3 still tests branch existence with 'git branch --list' (glob match)"
else
    pass "A7-no-branch-list" "step3 does not use 'git branch --list'"
fi

# A8: the helper is located exactly the way workflow-worktree.yaml step-04
# locates it, and the old installed-copies-first loop is gone.
A8_OK=1
for _want in \
    '${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/workflow_worktree_deconflict.sh' \
    '${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_worktree_deconflict.sh' \
    '$(pwd)/amplifier-bundle/tools/workflow_worktree_deconflict.sh' \
    '${HOME:-/root}/.copilot/amplifier-bundle/tools/workflow_worktree_deconflict.sh' \
    '${HOME:-/root}/.amplihack/amplifier-bundle/tools/workflow_worktree_deconflict.sh' \
    'DECONFLICT_HELPER="$TOOLS_DIR/workflow_worktree_deconflict.sh"' \
    '[ -f "$DECONFLICT_HELPER" ] || DECONFLICT_HELPER="${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_worktree_deconflict.sh"' \
    'if [ -f "$DECONFLICT_HELPER" ]'; do
    if ! grep -qF -- "${_want}" <<<"${STEP3}"; then
        A8_OK=0; fail "A8-step04-lookup" "step3 lacks step-04 lookup text: ${_want}"
    fi
done
for _bad in '[ -n "$DECONFLICT_HELPER" ]' 'for _candidate' 'on purpose'; do
    if grep -qF -- "${_bad}" <<<"${STEP3}"; then
        A8_OK=0; fail "A8-step04-lookup" "step3 still contains the old lookup: ${_bad}"
    fi
done
# The ladder must be in step-04's order: AMPLIHACK_HOME tier first, ~/.amplihack last.
_order="$(grep -nF -e '${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/workflow_worktree_deconflict.sh' \
                 -e '/.copilot/amplifier-bundle/tools/workflow_worktree_deconflict.sh' \
                 -e '/.amplihack/amplifier-bundle/tools/workflow_worktree_deconflict.sh' <<<"${STEP3}" \
          | cut -d: -f1 | tr '\n' ' ')"
read -r _o1 _o2 _o3 _rest <<<"${_order}"
if [[ -z "${_o3:-}" || -n "${_rest:-}" ]] || ! (( _o1 < _o2 && _o2 < _o3 )); then
    A8_OK=0; fail "A8-step04-lookup" "lookup tiers missing, repeated or out of step-04 order (lines: ${_order})"
fi
[[ ${A8_OK} -eq 1 ]] && pass "A8-step04-lookup" "step3 locates the helper with step-04's ladder and DECONFLICT_HELPER lines"

# ===========================================================================
# Part S — the real step3 bash against scratch repos.
# ===========================================================================
echo "=== Part S: step3 end-to-end ==="

# --- S0: fresh repo, helper present -> create ORIG, one JSON line on stdout.
R="$(build_repo s0)"
run_step "${R}"
if [[ ${RUN_RC} -eq 0 && "$(json_field branch_name)" == "${ORIG}" \
      && "$(json_field worktree_path)" == "$(wtdir "${R}")/${ORIG}" \
      && "$(json_field setup_complete)" == "true" ]] \
   && wt_registered "${R}" "$(wtdir "${R}")/${ORIG}"; then
    pass "S0-fresh-create" "fresh run creates ${ORIG} at WORKTREE_DIR/${ORIG} and reports it"
else
    fail "S0-fresh-create" "fresh run: rc=${RUN_RC} json='${RUN_JSON}' stderr: $(last_lines 3 "${RUN_STDERR}")"
fi
if [[ -n "${RUN_STDOUT}" && "$(printf '%s\n' "${RUN_STDOUT}" | wc -l)" -eq 1 && "${RUN_STDOUT}" == "${RUN_JSON}" ]]; then
    pass "S0-stdout-json-only" "stdout carries exactly the one JSON line"
else
    fail "S0-stdout-json-only" "stdout is not exactly one JSON line: '$(last_lines 3 "${RUN_STDOUT}")'"
fi

# --- S1: State 1, dirty and ahead, helper present -> work kept, fresh branch.
R="$(build_repo s1)"; state1 "${R}"
W="$(wtdir "${R}")/${ORIG}"
commit_ahead "${W}"
KEPT_SHA="$(git -C "${R}" rev-parse "refs/heads/${ORIG}")"
make_dirty "${W}"
SUM_AHEAD="$(file_sum "${W}/ahead.txt")"
SUM_README="$(file_sum "${W}/README.md")"
SUM_UNTRACKED="$(file_sum "${W}/untracked.txt")"
run_step "${R}"
NEW_BRANCH="$(json_field branch_name)"
NEW_WT="$(json_field worktree_path)"
if [[ ${RUN_RC} -eq 0 ]]; then
    pass "S1-exit0" "dirty+ahead State 1 re-run exits 0"
else
    fail "S1-exit0" "dirty+ahead State 1 re-run exited ${RUN_RC}: $(last_lines 3 "${RUN_STDERR}")"
fi
if [[ "$(file_sum "${W}/untracked.txt")" == "${SUM_UNTRACKED}" ]]; then
    pass "S1-untracked-survives" "untracked file is byte-identical"
else
    fail "S1-untracked-survives" "untracked file was deleted or changed (#1626)"
fi
if [[ "$(file_sum "${W}/README.md")" == "${SUM_README}" ]]; then
    pass "S1-modified-survives" "modified tracked file is byte-identical"
else
    fail "S1-modified-survives" "modified tracked file was reverted (#1626)"
fi
if [[ "$(file_sum "${W}/ahead.txt")" == "${SUM_AHEAD}" \
      && "$(git -C "${R}" rev-parse "refs/heads/${ORIG}" 2>/dev/null)" == "${KEPT_SHA}" \
      && "$(git -C "${W}" rev-parse HEAD 2>/dev/null)" == "${KEPT_SHA}" ]]; then
    pass "S1-commit-survives" "unpushed commit ${KEPT_SHA:0:12} is still the branch and worktree HEAD"
else
    fail "S1-commit-survives" "unpushed commit was orphaned or the branch moved (#1626)"
fi
if [[ -n "${NEW_BRANCH}" && "${NEW_BRANCH}" != "${ORIG}" && "${NEW_BRANCH}" == "${ORIG}-"* ]]; then
    pass "S1-new-branch" "run moved to its own branch '${NEW_BRANCH}'"
else
    fail "S1-new-branch" "expected branch_name '${ORIG}-<suffix>', got '${NEW_BRANCH}'"
fi
if [[ -n "${NEW_BRANCH}" && "${NEW_WT}" == "$(wtdir "${R}")/${NEW_BRANCH}" ]] \
   && wt_registered "${R}" "${NEW_WT}" \
   && [[ "$(git -C "${NEW_WT}" symbolic-ref --short HEAD 2>/dev/null)" == "${NEW_BRANCH}" \
         && "$(git -C "${NEW_WT}" rev-parse HEAD 2>/dev/null)" == "$(git -C "${R}" rev-parse origin/main)" ]]; then
    pass "S1-new-worktree" "worktree_path is WORKTREE_DIR/<new>, registered, on the new branch at BASE_REF"
else
    fail "S1-new-worktree" "worktree_path '${NEW_WT}' is not a registered WORKTREE_DIR/${NEW_BRANCH:-<new>} at BASE_REF"
fi
if ! git -C "${R}-origin.git" cat-file -e "${KEPT_SHA}^{commit}" 2>/dev/null; then
    pass "S1-not-pushed" "the unpushed commit did not reach origin"
else
    fail "S1-not-pushed" "unpushed commit ${KEPT_SHA:0:12} was pushed to origin"
fi
# push_checks <id> <repo> <new-branch>: the push follows the resolved branch.
# The suffixed branch reaches origin at BASE_REF; the original never does.
push_checks() {
    local id="$1" r="$2" nb="$3" remote_new remote_orig base
    base="$(git -C "${r}" rev-parse origin/main)"
    remote_new=""
    [[ -n "${nb}" ]] && remote_new="$(git -C "${r}" ls-remote origin "refs/heads/${nb}" 2>/dev/null | awk '{print $1}')"
    remote_orig="$(git -C "${r}" ls-remote origin "refs/heads/${ORIG}" 2>/dev/null)"
    if [[ -n "${nb}" && "${nb}" != "${ORIG}" && "${remote_new}" == "${base}" ]]; then
        pass "${id}-push-suffixed" "origin has refs/heads/${nb} at BASE_REF"
    else
        fail "${id}-push-suffixed" "origin refs/heads/${nb:-<new>} is '${remote_new}', expected BASE_REF ${base:0:12}"
    fi
    if [[ -z "${remote_orig}" ]]; then
        pass "${id}-push-not-orig" "origin has no refs/heads/${ORIG}"
    else
        fail "${id}-push-not-orig" "the original branch was pushed to origin: ${remote_orig}"
    fi
}
push_checks S1 "${R}" "${NEW_BRANCH}"

# --- S2: State 2 (branch ahead, worktree removed), helper present.
R="$(build_repo s2)"; state1 "${R}"
W="$(wtdir "${R}")/${ORIG}"
commit_ahead "${W}"
KEPT_SHA="$(git -C "${R}" rev-parse "refs/heads/${ORIG}")"
git -C "${R}" worktree remove -- "${W}"
run_step "${R}"
NEW_BRANCH="$(json_field branch_name)"
if [[ "$(git -C "${R}" rev-parse --verify --quiet "refs/heads/${ORIG}" 2>/dev/null)" == "${KEPT_SHA}" ]]; then
    pass "S2-branch-survives" "ahead branch still points at ${KEPT_SHA:0:12}"
else
    fail "S2-branch-survives" "ahead branch was deleted or moved (State 2 'branch -D') (#1626)"
fi
if [[ ${RUN_RC} -eq 0 && "${NEW_BRANCH}" == "${ORIG}-"* && "${NEW_BRANCH}" != "${ORIG}" \
      && "$(json_field worktree_path)" == "$(wtdir "${R}")/${NEW_BRANCH}" ]]; then
    pass "S2-new-branch" "run moved to its own branch '${NEW_BRANCH}'"
else
    fail "S2-new-branch" "expected a '${ORIG}-<suffix>' branch, got '${NEW_BRANCH}' rc=${RUN_RC}"
fi
push_checks S2 "${R}" "${NEW_BRANCH}"

# --- S3: clean State 1, helper present -> reused unchanged.
R="$(build_repo s3)"; state1 "${R}"
W="$(wtdir "${R}")/${ORIG}"
SHA_BEFORE="$(git -C "${R}" rev-parse "refs/heads/${ORIG}")"
run_step "${R}"
if [[ ${RUN_RC} -eq 0 && "$(json_field branch_name)" == "${ORIG}" \
      && "$(json_field worktree_path)" == "${W}" \
      && "$(git -C "${R}" rev-parse "refs/heads/${ORIG}")" == "${SHA_BEFORE}" ]]; then
    pass "S3-clean-reuse" "clean State 1 reused: same branch, same worktree path"
else
    fail "S3-clean-reuse" "clean State 1 not reused: rc=${RUN_RC} json='${RUN_JSON}'"
fi

# --- S4: clean State 2, helper present -> worktree re-attached, same name.
R="$(build_repo s4)"
git -C "${R}" branch -q "${ORIG}" origin/main
SHA_BEFORE="$(git -C "${R}" rev-parse "refs/heads/${ORIG}")"
run_step "${R}"
W="$(wtdir "${R}")/${ORIG}"
if [[ ${RUN_RC} -eq 0 && "$(json_field branch_name)" == "${ORIG}" && "$(json_field worktree_path)" == "${W}" ]] \
   && wt_registered "${R}" "${W}" \
   && [[ "$(git -C "${W}" rev-parse HEAD 2>/dev/null)" == "${SHA_BEFORE}" ]]; then
    pass "S4-clean-attach" "clean leftover branch is re-attached unchanged"
else
    fail "S4-clean-attach" "clean State 2 not re-attached: rc=${RUN_RC} json='${RUN_JSON}'"
fi

# --- Helper hidden: AMPLIHACK_HOME empty, no HOME/.amplihack, none in REPO_PATH.
[[ ! -e "${HOME}/.amplihack" ]] || harness "HOME/.amplihack must not exist"

# --- S5: no helper, State 1 dirty and ahead -> refuse, nothing touched.
R="$(build_repo s5)"; state1 "${R}"
W="$(wtdir "${R}")/${ORIG}"
commit_ahead "${W}"; make_dirty "${W}"
[[ ! -e "${R}/${HELPER_REL}" ]] || harness "S5 helper is not hidden"
SUM_ALL="$(file_sum "${W}/ahead.txt") $(file_sum "${W}/README.md") $(file_sum "${W}/untracked.txt")"
HEADS_BEFORE="$(heads "${R}")"; WTS_BEFORE="$(wt_list "${R}")"
run_step "${R}" "${AHOME_NONE}"
if [[ ${RUN_RC} -ne 0 ]] && grep -q 'ERROR' <<<"${RUN_STDERR}"; then
    pass "S5-refuses" "missing helper + dirty/ahead State 1 exits ${RUN_RC} with ERROR"
else
    fail "S5-refuses" "missing helper + dirty/ahead State 1: rc=${RUN_RC}, expected non-zero with ERROR"
fi
if [[ "$(file_sum "${W}/ahead.txt") $(file_sum "${W}/README.md") $(file_sum "${W}/untracked.txt")" == "${SUM_ALL}" \
      && "$(heads "${R}")" == "${HEADS_BEFORE}" && "$(wt_list "${R}")" == "${WTS_BEFORE}" ]]; then
    pass "S5-intact" "files, branch SHAs and worktree list are unchanged"
else
    fail "S5-intact" "work, a branch or the worktree list changed although the step refused"
fi

# --- S6: no helper, State 2 ahead -> refuse, branch intact, no worktree.
R="$(build_repo s6)"; state1 "${R}"
W="$(wtdir "${R}")/${ORIG}"
commit_ahead "${W}"
git -C "${R}" worktree remove -- "${W}"
HEADS_BEFORE="$(heads "${R}")"; WTS_BEFORE="$(wt_list "${R}")"
run_step "${R}" "${AHOME_NONE}"
if [[ ${RUN_RC} -ne 0 ]] && grep -q 'ERROR' <<<"${RUN_STDERR}"; then
    pass "S6-refuses" "missing helper + ahead State 2 exits ${RUN_RC} with ERROR"
else
    fail "S6-refuses" "missing helper + ahead State 2: rc=${RUN_RC}, expected non-zero with ERROR"
fi
if [[ "$(heads "${R}")" == "${HEADS_BEFORE}" && "$(wt_list "${R}")" == "${WTS_BEFORE}" ]]; then
    pass "S6-intact" "branch SHAs unchanged and no worktree added"
else
    fail "S6-intact" "a branch moved or a worktree was added although the step refused"
fi

# --- S7: no helper, clean State 1 -> reused, exit 0.
R="$(build_repo s7)"; state1 "${R}"
run_step "${R}" "${AHOME_NONE}"
if [[ ${RUN_RC} -eq 0 && "$(json_field branch_name)" == "${ORIG}" \
      && "$(json_field worktree_path)" == "$(wtdir "${R}")/${ORIG}" ]]; then
    pass "S7-nohelper-clean-reuse" "missing helper does not block reusing a clean branch"
else
    fail "S7-nohelper-clean-reuse" "missing helper + clean State 1: rc=${RUN_RC} json='${RUN_JSON}'"
fi

# --- S7b: no helper, fresh repo -> created, exit 0 (helper is best-effort).
R="$(build_repo s7b)"
run_step "${R}" "${AHOME_NONE}"
if [[ ${RUN_RC} -eq 0 && "$(json_field branch_name)" == "${ORIG}" ]] \
   && wt_registered "${R}" "$(wtdir "${R}")/${ORIG}"; then
    pass "S7b-nohelper-fresh" "missing helper does not block a fresh run"
else
    fail "S7b-nohelper-fresh" "missing helper + fresh repo: rc=${RUN_RC} json='${RUN_JSON}'"
fi

# --- Untrusted helper output.
# s8_check <id> <ahome> <what> -> the step must refuse and change nothing.
s8_check() {
    local id="$1" ahome="$2" what="$3" r hb wb
    r="$(build_repo "${id}")"; state1 "${r}"
    hb="$(heads "${r}")"; wb="$(wt_list "${r}")"
    run_step "${r}" "${ahome}"
    if [[ ${RUN_RC} -ne 0 ]] && grep -q 'ERROR' <<<"${RUN_STDERR}"; then
        pass "${id}-rejected" "${what}: step exits ${RUN_RC} with ERROR"
    else
        fail "${id}-rejected" "${what}: rc=${RUN_RC}, expected non-zero with ERROR"
    fi
    if [[ "$(heads "${r}")" == "${hb}" && "$(wt_list "${r}")" == "${wb}" ]]; then
        pass "${id}-intact" "${what}: no branch moved or created, no worktree added"
    else
        fail "${id}-intact" "${what}: refs or worktrees changed"
    fi
}

# S8: helper names an unrelated existing branch.
s8_check S8 "$(make_stub main "printf '%s\n' main")" "helper answer 'main'"
# S9: a quote passes check-ref-format but would forge the JSON line.
s8_check S9 "$(make_stub quote "printf '%s\n' '${ORIG}-a\"b'")" "helper answer '${ORIG}-a\"b'"
# S10: helper exits non-zero -> 'deconfliction failed'.
STUB_FAIL="$(make_stub fail 'echo "helper: exhausted" >&2; exit 1')"
s8_check S10 "${STUB_FAIL}" "helper exit 1"
if grep -qF 'deconfliction failed' <<<"${RUN_STDERR}"; then
    pass "S10-message" "stderr names 'deconfliction failed'"
else
    fail "S10-message" "stderr lacks 'deconfliction failed': $(last_lines 2 "${RUN_STDERR}")"
fi

# --- S11: trusted helper wins over one planted in the repo (SR-12).
R="$(build_repo s11)"; state1 "${R}"
mkdir -p "${R}/amplifier-bundle/tools"
printf '#!/usr/bin/env bash\nprintf "%%s\\n" main\n' > "${R}/${HELPER_REL}"
run_step "${R}" "${AHOME_OK}"
if [[ ${RUN_RC} -eq 0 && "$(json_field branch_name)" == "${ORIG}" ]]; then
    pass "S11-trusted-first" "AMPLIHACK_HOME's helper is used before the repo-local copy"
else
    fail "S11-trusted-first" "repo-local helper took precedence or the run failed: rc=${RUN_RC} json='${RUN_JSON}'"
fi

# --- S11b: AMPLIHACK_HOME empty -> step-04's ladder reaches the helper in the
# repo. (a) it is really used; (b) its answer is still validated.
PLANTED_BRANCH="feat/consensus-fix-issue-1626-planted"
[[ "${PLANTED_BRANCH}" == "${ORIG}-planted" ]] || harness "S11b planted name drifted from ORIG"

# s11b_repo <id> -> dirty+ahead State 1 repo with the planted helper written by
# the caller; sets R, W, SUM_ALL, HEAD_BEFORE, HEADS_BEFORE, WTS_BEFORE.
s11b_repo() {
    R="$(build_repo "$1")"; state1 "${R}"
    W="$(wtdir "${R}")/${ORIG}"
    commit_ahead "${W}"; make_dirty "${W}"
    mkdir -p "${R}/amplifier-bundle/tools"
}
s11b_snapshot() {
    SUM_ALL="$(file_sum "${W}/ahead.txt") $(file_sum "${W}/README.md") $(file_sum "${W}/untracked.txt")"
    HEAD_BEFORE="$(git -C "${W}" rev-parse HEAD)"
    HEADS_BEFORE="$(heads "${R}")"; WTS_BEFORE="$(wt_list "${R}")"
}
s11b_orig_intact() {
    [[ "$(file_sum "${W}/ahead.txt") $(file_sum "${W}/README.md") $(file_sum "${W}/untracked.txt")" == "${SUM_ALL}" \
       && "$(git -C "${W}" rev-parse HEAD)" == "${HEAD_BEFORE}" \
       && "$(git -C "${R}" rev-parse "refs/heads/${ORIG}")" == "${HEAD_BEFORE}" ]]
}

# S11b(a): the planted helper answers <ORIG>-planted.
s11b_repo s11ba
cat > "${R}/${HELPER_REL}" <<'HELPER_EOF'
#!/usr/bin/env bash
printf '%s\n' 'feat/consensus-fix-issue-1626-planted'
HELPER_EOF
s11b_snapshot
run_step "${R}" ""
if [[ ${RUN_RC} -eq 0 && "$(json_field branch_name)" == "${PLANTED_BRANCH}" \
      && "$(json_field worktree_path)" == "$(wtdir "${R}")/${PLANTED_BRANCH}" ]] \
   && wt_registered "${R}" "$(wtdir "${R}")/${PLANTED_BRANCH}" \
   && [[ "$(git -C "$(wtdir "${R}")/${PLANTED_BRANCH}" branch --show-current 2>/dev/null)" == "${PLANTED_BRANCH}" ]]; then
    pass "S11b-a-repo-helper-used" "AMPLIHACK_HOME empty: the repo's helper is used and the outputs follow '${PLANTED_BRANCH}'"
else
    fail "S11b-a-repo-helper-used" "AMPLIHACK_HOME empty, repo helper: rc=${RUN_RC} json='${RUN_JSON}' stderr: $(last_lines 2 "${RUN_STDERR}")"
fi
if s11b_orig_intact; then
    pass "S11b-a-intact" "original worktree's files and HEAD are unchanged"
else
    fail "S11b-a-intact" "original worktree's files or HEAD changed"
fi

# S11b(b): the planted helper answers a name without the ORIG prefix.
s11b_repo s11bb
cat > "${R}/${HELPER_REL}" <<'HELPER_EOF'
#!/usr/bin/env bash
printf '%s\n' 'other'
HELPER_EOF
s11b_snapshot
run_step "${R}" ""
if [[ ${RUN_RC} -ne 0 ]] && grep -q 'ERROR' <<<"${RUN_STDERR}"; then
    pass "S11b-b-rejected" "repo helper answer 'other' is refused: exit ${RUN_RC} with ERROR"
else
    fail "S11b-b-rejected" "repo helper answer 'other': rc=${RUN_RC}, expected non-zero with ERROR"
fi
if s11b_orig_intact && [[ "$(heads "${R}")" == "${HEADS_BEFORE}" && "$(wt_list "${R}")" == "${WTS_BEFORE}" ]] \
   && ! git -C "${R}" show-ref --verify --quiet refs/heads/other; then
    pass "S11b-b-intact" "files, branch heads and worktree list are unchanged"
else
    fail "S11b-b-intact" "work, a branch or the worktree list changed although the step refused"
fi

# --- S12: a worktree path that could forge the JSON line is refused before
# any branch, worktree or push is made (fresh repo, helper present).
R="$(build_repo s12)"
BAD_WTD="${SCRATCH}/s12-wts/wt\"x"
HEADS_BEFORE="$(heads "${R}")"; WTS_BEFORE="$(wt_list "${R}")"
run_step "${R}" "${AHOME_OK}" "${BAD_WTD}"
if [[ ${RUN_RC} -ne 0 ]] && grep -qF 'refusing to emit setup JSON' <<<"${RUN_STDERR}"; then
    pass "S12-refuses" "worktree path containing '\"' exits ${RUN_RC} with 'refusing to emit setup JSON'"
else
    fail "S12-refuses" "bad worktree path: rc=${RUN_RC}, stderr: $(last_lines 2 "${RUN_STDERR}")"
fi
if ! git -C "${R}" show-ref --verify --quiet "refs/heads/${ORIG}" \
   && [[ -z "$(git -C "${R}" ls-remote origin "refs/heads/${ORIG}" 2>/dev/null)" ]] \
   && [[ "$(heads "${R}")" == "${HEADS_BEFORE}" && "$(wt_list "${R}")" == "${WTS_BEFORE}" ]] \
   && [[ ! -e "${BAD_WTD}" ]]; then
    pass "S12-before-change" "no branch, worktree or pushed ref was created before the refusal"
else
    fail "S12-before-change" "the step created a branch, worktree or pushed ref before refusing the path"
fi

# --- S13: work still turns up after deconfliction. The helper answers a valid
# suffixed name, <ORIG>-busy, that itself holds work. The guard runs on the
# resolved name and must refuse; neither branch is touched or pushed.
BUSY="${ORIG}-busy"
STUB_BUSY="$(make_stub busy "printf '%s\\n' '${BUSY}'")"

# s13_sums: checksums of the tracked, modified and untracked files in W and,
# when it exists, BW.
s13_sums() {
    local d
    for d in "${W}" "${BW}"; do
        [[ -d "${d}" ]] || continue
        printf '%s %s %s ' "$(file_sum "${d}/ahead.txt")" "$(file_sum "${d}/README.md")" "$(file_sum "${d}/untracked.txt")"
    done
}

# s13_check <id> <changes yes|no>: R, W (ORIG worktree) and BW (BUSY path) set.
s13_check() {
    local id="$1" changes="$2" sums_b heads_b wts_b remote_b orig_head busy_head busy_wt_head=""
    sums_b="$(s13_sums)"
    heads_b="$(heads "${R}")"; wts_b="$(wt_list "${R}")"
    remote_b="$(git -C "${R}" ls-remote origin 2>/dev/null)"
    orig_head="$(git -C "${W}" rev-parse HEAD)"
    busy_head="$(git -C "${R}" rev-parse "refs/heads/${BUSY}")"
    [[ -d "${BW}" ]] && busy_wt_head="$(git -C "${BW}" rev-parse HEAD)"

    run_step "${R}" "${STUB_BUSY}"

    if [[ ${RUN_RC} -ne 0 ]]; then
        pass "${id}-exit" "deconflicted branch holding work: step exits ${RUN_RC}"
    else
        fail "${id}-exit" "deconflicted branch holding work: step exited 0 (json='${RUN_JSON}')"
    fi
    if grep -qF 'ERROR' <<<"${RUN_STDERR}" \
       && grep -qF "existing_branch=${BUSY} " <<<"${RUN_STDERR}" \
       && grep -qF "uncommitted/untracked changes: ${changes}, commits ahead of origin/main: 1" <<<"${RUN_STDERR}"; then
        pass "${id}-error" "ERROR names '${BUSY}' with changes: ${changes}, ahead: 1"
    else
        fail "${id}-error" "stderr lacks the refusal for '${BUSY}' (changes: ${changes}): $(last_lines 2 "${RUN_STDERR}")"
    fi
    if [[ "$(s13_sums)" == "${sums_b}" ]]; then
        pass "${id}-files" "untracked, modified and committed files in both worktrees are byte-identical"
    else
        fail "${id}-files" "a file in '${ORIG}' or '${BUSY}' was changed or deleted"
    fi
    if [[ "$(heads "${R}")" == "${heads_b}" \
          && "$(git -C "${W}" rev-parse HEAD)" == "${orig_head}" \
          && "$(git -C "${R}" rev-parse "refs/heads/${BUSY}" 2>/dev/null)" == "${busy_head}" ]] \
       && { [[ -z "${busy_wt_head}" ]] || [[ "$(git -C "${BW}" rev-parse HEAD 2>/dev/null)" == "${busy_wt_head}" ]]; }; then
        pass "${id}-refs" "branch heads and worktree HEADs are unchanged"
    else
        fail "${id}-refs" "a branch or worktree HEAD moved although the step refused"
    fi
    if [[ "$(wt_list "${R}")" == "${wts_b}" ]]; then
        pass "${id}-wts" "git worktree list is unchanged"
    else
        fail "${id}-wts" "a worktree was added or removed although the step refused"
    fi
    if [[ "$(git -C "${R}" ls-remote origin 2>/dev/null)" == "${remote_b}" ]] \
       && [[ -z "$(git -C "${R}" ls-remote origin "refs/heads/${ORIG}" "refs/heads/${BUSY}" 2>/dev/null)" ]]; then
        pass "${id}-no-push" "git ls-remote origin is identical; neither branch was pushed"
    else
        fail "${id}-no-push" "origin's refs changed although the step refused"
    fi
    if ! grep -qF 'setup_complete' <<<"${RUN_STDOUT}"; then
        pass "${id}-no-json" "no setup JSON line on stdout"
    else
        fail "${id}-no-json" "stdout carries a setup JSON line although the step refused: '${RUN_JSON}'"
    fi
}

# S13-state1: <ORIG>-busy has a worktree at exactly WORKTREE_DIR/<ORIG>-busy,
# one commit ahead, a modified tracked file and an untracked file.
R="$(build_repo s13-state1)"; state1 "${R}"
W="$(wtdir "${R}")/${ORIG}"
commit_ahead "${W}"; make_dirty "${W}"
BW="$(wtdir "${R}")/${BUSY}"
git -C "${R}" worktree add -q -b "${BUSY}" -- "${BW}" origin/main
commit_ahead "${BW}"; make_dirty "${BW}"
wt_registered "${R}" "${BW}" || harness "S13-state1 busy worktree is not at WORKTREE_DIR/${BUSY}"
s13_check S13-state1 yes

# S13-state2: <ORIG>-busy is a branch only, one commit ahead.
R="$(build_repo s13-state2)"; state1 "${R}"
W="$(wtdir "${R}")/${ORIG}"
commit_ahead "${W}"; make_dirty "${W}"
BW="$(wtdir "${R}")/${BUSY}"
_tmpwt="${SCRATCH}/s13-state2-tmpwt"
git -C "${R}" worktree add -q -b "${BUSY}" -- "${_tmpwt}" origin/main
commit_ahead "${_tmpwt}"
git -C "${R}" worktree remove -- "${_tmpwt}"
if wt_list "${R}" | grep -qF -- "${BUSY}" || wt_registered "${R}" "${_tmpwt}" || [[ -e "${BW}" ]]; then
    harness "S13-state2 busy branch still has a worktree; scenario would not be state 2"
fi
s13_check S13-state2 no

# ===========================================================================
echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"
if [[ ${FAIL_COUNT} -gt 0 ]]; then
    exit 1
fi
echo "PASS: Issue #1626 — re-running consensus step3 never destroys an existing branch's work."
exit 0
