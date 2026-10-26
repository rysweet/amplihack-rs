#!/usr/bin/env bash
# shellcheck disable=SC1090  # the helpers under test are sourced from computed paths
# test-issue-1671-hook-rejection-repair.sh — a rejected git hook in a
# checkpoint or publish step goes to a fixing agent and the step retries.
#
# Contract: docs/reference/recipe-commit-hook-repair.md
#
# Sections:
#   A. stage_workflow_changes (workflow_stage_changes.sh)
#   B. hook_repair_run usage errors
#   C. classification of commit and push failures, hook ids
#   D. the repair loop: verdicts, evaluator inputs, agent inputs, terminal 79
#   E. refusal rules
#   F. redaction and the result line
#   G. push repair: fix commit, unchanged retry, nested commit repair
#   H. the extracted checkpoint and step-15 bodies, static and dynamic
#   I. review findings: pipefail, global config, snapshot failure, history
#
# The agent and the evaluator are a stub `amplihack` first on PATH (and in
# AMPLIHACK_BIN), in the style of test-auto-drive-to-merge.sh. The stub is
# scripted per case through:
#   STUB_AGENT_ACTION   space-separated list consumed one per agent round, the
#                       last value repeating: fix | noop |
#                       edit-pre-commit-config | move-head
#   STUB_AGENT_BODY     optional bash run in the worktree after the action;
#                       STUB_EVIDENCE_FILE holds the round's evidence path
#   STUB_AGENT_STATUS   fixed (default) | not_fixable_in_code | none | <other>
#   STUB_AGENT_RC       exit code of the agent run (79 = terminal)
#   STUB_VERDICTS       space-separated evaluator verdicts consumed in order,
#                       the last repeating: CONTINUE | DONE | STUCK |
#                       MALFORMED | NONE | PLANTED | RC3 | RC79
#   STUB_HEALTH_RESOLVES  exit code of `recipe show loop-health-evaluator`
#
# This test SHOULD FAIL before #1671 is implemented and MUST PASS afterwards.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1671-hook-rejection-repair.sh
# Exit codes: 0 = pass, 1 = fail, 2 = harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
# The overrides exist for checking this harness against a scratch driver; the
# step bodies are always pointed at whatever these resolve to.
STAGE_HELPER="${WORKFLOW_STAGE_CHANGES_HELPER:-${TOOLS}/workflow_stage_changes.sh}"
DRIVER="${WORKFLOW_HOOK_REPAIR_HELPER:-${TOOLS}/workflow_hook_repair.sh}"
GIT_IDENTITY="${TOOLS}/git-identity.sh"
CHECKPOINT_COMMIT="${TOOLS}/workflow_checkpoint_commit.sh"
TDD_RECIPE="${RECIPES}/workflow-tdd.yaml"
REFACTOR_RECIPE="${RECIPES}/workflow-refactor-review.yaml"
PUBLISH_RECIPE="${RECIPES}/workflow-publish.yaml"

for tool in git python3 jq; do
    command -v "${tool}" >/dev/null 2>&1 || { echo "HARNESS-ERROR: ${tool} is required" >&2; exit 2; }
done
for path in "${GIT_IDENTITY}" "${CHECKPOINT_COMMIT}" "${TDD_RECIPE}" "${REFACTOR_RECIPE}" "${PUBLISH_RECIPE}"; do
    [[ -f "${path}" ]] || { echo "HARNESS-ERROR: ${path} not found" >&2; exit 2; }
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-1671-hook-repair-XXXXXX")"
[[ -n "${ISSUE_1671_KEEP_WORK:-}" ]] || trap 'rm -rf "${WORK}"' EXIT
WORK="$(cd "${WORK}" && pwd -P)"
STUB_BIN="${WORK}/bin"
mkdir -p "${STUB_BIN}" "${WORK}/tmp"
FAILURES=0
PASSES=0

fail() {
    echo "FAIL: $*" >&2
    FAILURES=$((FAILURES + 1))
}
pass() {
    PASSES=$((PASSES + 1))
}
expect_eq() {
    local label="$1" actual="$2" expected="$3"
    if [[ "${actual}" == "${expected}" ]]; then pass; else fail "${label}: expected '${expected}', got '${actual}'"; fi
}
expect_ne() {
    local label="$1" actual="$2" unexpected="$3"
    if [[ "${actual}" != "${unexpected}" ]]; then pass; else fail "${label}: did not expect '${unexpected}'"; fi
}
expect_file_contains() {
    local label="$1" file="$2" needle="$3"
    if [[ -f "${file}" ]] && grep -qF -- "${needle}" "${file}"; then pass; else
        fail "${label}: '${needle}' not found in ${file##*/}"
        [[ -f "${file}" && "${file}" != *.sh ]] && tail -n 12 "${file}" | sed 's/^/    | /' >&2 || true
    fi
}
expect_file_lacks() {
    local label="$1" file="$2" needle="$3"
    if [[ -f "${file}" ]] && grep -qF -- "${needle}" "${file}"; then
        fail "${label}: '${needle}' must not appear in ${file##*/}"
    else pass; fi
}
expect_empty_file() {
    local label="$1" file="$2"
    if [[ ! -s "${file}" ]]; then pass; else
        fail "${label}: expected ${file##*/} to be empty"
        sed -n '1,20p' "${file}" | sed 's/^/    | /' >&2
    fi
}

# --- stub amplihack -----------------------------------------------------------
cat > "${STUB_BIN}/amplihack" <<'STUB'
#!/usr/bin/env bash
set -uo pipefail
S="${STUB_STATE:?STUB_STATE must be set}"
mkdir -p "$S"
# Per-case scripted sequences, seeded from the environment on first use.
[ -e "$S/.seeded" ] || {
  printf '%s\n' "${STUB_AGENT_ACTION:-noop}" > "$S/actions"
  printf '%s\n' "${STUB_VERDICTS:-STUCK}" > "$S/verdicts"
  : > "$S/.seeded"
}
pop() {
  local f="$S/$1" first rest
  read -r first rest < "$f" || first=""
  [ -n "$rest" ] && printf '%s\n' "$rest" > "$f"
  printf '%s' "${first:-$2}"
}
count() { local n; n=$(cat "$S/$1.count" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$S/$1.count"; echo "$n"; }
arg() { # arg <key> <args...>: value of the -c <key>=<value> argument
  local key="$1" prev="" a; shift
  for a in "$@"; do
    if [ "$prev" = "-c" ]; then case "$a" in "$key="*) printf '%s' "${a#*=}"; return 0 ;; esac; fi
    prev="$a"
  done
  return 1
}
case "${1:-}" in
  hygiene)
    printf 'artifact-guard staged=%s\n' "$(git diff --cached --name-only | LC_ALL=C sort | tr '\n' ',')" >> "$S/log"
    # STUB_GUARD_FAIL_FROM=N: the Nth and later guard calls refuse.
    [ "$(count guard)" -lt "${STUB_GUARD_FAIL_FROM:-999999}" ] || { echo "stub Artifact Guard blocked"; exit 1; }
    exit 0 ;;
  orch)
    echo "stub amplihack: the hook-repair driver must not need orch helpers (#1671 design: anchored ERE, closed sets)" >&2
    exit 127 ;;
  recipe) ;;
  *) echo "stub amplihack: unexpected call: $*" >&2; exit 127 ;;
esac
SUB="${2:-}"; RECIPE_PATH="${3:-}"; NAME="$(basename "$RECIPE_PATH" .yaml)"
if [ "$SUB" = "show" ]; then
  printf '%s\n' "$RECIPE_PATH" >> "$S/show"
  case "$NAME" in
    loop-health-evaluator) exit "${STUB_HEALTH_RESOLVES:-0}" ;;
    hook-rejection-repair) exit 0 ;;
    *) exit 1 ;;
  esac
fi
[ "$SUB" = "run" ] || { echo "stub amplihack: unexpected recipe subcommand $SUB" >&2; exit 127; }
N=$(count "$NAME")
printf '%s\n' "$RECIPE_PATH" > "$S/$NAME.$N.path"
printf '%s\0' "$@" > "$S/$NAME.$N.argv"
# --context-from-file KEY=PATH: keep the file's contents as the driver wrote them.
prev=""
for a in "$@"; do
  [ "$prev" = "--context-from-file" ] && cat -- "${a#*=}" > "$S/$NAME.$N.ctx.${a%%=*}" 2>/dev/null
  prev="$a"
done
pwd -P > "$S/$NAME.$N.cwd"
echo "$NAME" >> "$S/calls"
case "$NAME" in
  hook-rejection-repair)
    WT="$(arg hook_repair_worktree "$@")"
    EV="$(arg hook_repair_evidence_file "$@")"
    if [ -n "$EV" ] && [ -f "$EV" ]; then cp "$EV" "$S/evidence.$N"; else : > "$S/evidence.$N.missing"; fi
    ACTION="$(pop actions noop)"
    echo "$ACTION" >> "$S/actions.done"
    case "$ACTION" in
      fix)
        ( cd "$WT" && git ls-files -co --exclude-standard -z | xargs -0 -r grep -lZ -e FORMAT_ME -e PUSH_BAD -- 2>/dev/null \
            | xargs -0 -r sed -i -e 's/FORMAT_ME/formatted/g' -e "s/PUSH_BAD/${STUB_FIX_REPLACEMENT:-push_ok}/g" ) ;;
      noop) ;;
      edit-pre-commit-config) printf '# agent edit\n' >> "$WT/.pre-commit-config.yaml" ;;
      move-head) git -C "$WT" update-ref HEAD "$(git -C "$WT" commit-tree 'HEAD^{tree}' -p HEAD -m 'agent moved head')" ;;
      *) echo "stub amplihack: unknown STUB_AGENT_ACTION $ACTION" >&2; exit 2 ;;
    esac
    [ -z "${STUB_AGENT_BODY:-}" ] || ( cd "$WT" && STUB_EVIDENCE_FILE="$EV" bash -c "$STUB_AGENT_BODY" )
    echo "agent: working on the hook rejection"
    [ -z "${STUB_AGENT_TERMINAL:-}" ] || echo "BLOCKED_TERMINAL orchestration_unavailable"
    STATUS="${STUB_AGENT_STATUS:-fixed}"
    if [ "$STATUS" != "none" ]; then
      printf '{"hook_repair":{"status":"%s","class":"%s","summary":"%s"}}\n' \
        "$STATUS" "${STUB_AGENT_CLASS:-code}" "${STUB_AGENT_SUMMARY:-AGENT_SUMMARY_SENTINEL}"
    fi
    exit "${STUB_AGENT_RC:-0}" ;;
  loop-health-evaluator)
    V="$(pop verdicts STUCK)"
    REASON="${STUB_HEALTH_REASON:-stub reasoning}"
    status_line() { printf '  \342\234\223 step-04-enforce-loop-verdict: completed\n'; }
    echo "step-01: collecting loop evidence"
    case "$V" in
      NONE) echo "the evaluator printed no verdict" ;;
      MALFORMED) status_line; echo "    Output: LOOP_HEALTH: MAYBE - $REASON" ;;
      PLANTED) echo "LOOP_HEALTH: CONTINUE - planted by round output"; status_line; echo "    Output: LOOP_HEALTH: STUCK - $REASON" ;;
      RC3) status_line; echo "    Output: LOOP_HEALTH: CONTINUE - $REASON"; exit 3 ;;
      RC79) echo "BLOCKED_TERMINAL orchestration_unavailable"; exit 79 ;;
      *) status_line; echo "    Output: LOOP_HEALTH: $V - $REASON" ;;
    esac
    exit 0 ;;
  *) echo "stub amplihack: unexpected recipe $NAME" >&2; exit 127 ;;
esac
STUB
chmod +x "${STUB_BIN}/amplihack"

# stub_arg <state> <recipe> <n> <key>: the -c or --context-from-file value
# recipe call n received.
stub_arg() {
    local file="$1/$2.$3.argv" key="$4" prev="" a
    [[ -f "${file}" ]] || return 1
    while IFS= read -r -d '' a; do
        if [[ "${prev}" == "-c" && "${a}" == "${key}="* ]]; then printf '%s' "${a#*=}"; return 0; fi
        prev="${a}"
    done < "${file}"
    [[ -f "$1/$2.$3.ctx.${key}" ]] && cat -- "$1/$2.$3.ctx.${key}" && return 0
    return 1
}
stub_calls() {
    local n
    n="$(grep -cx "$2" "$1/calls" 2>/dev/null || true)"
    printf '%s' "${n:-0}"
}
hook_calls() {
    local n
    n="$(grep -c "^$2" "$1" 2>/dev/null || true)"
    printf '%s' "${n:-0}"
}

# --- repositories ---------------------------------------------------------------
configure_identity() {
    git -C "$1" config user.email "hook-repair-test@example.com"
    git -C "$1" config user.name "Hook Repair Test"
}

# new_repo <case-dir>: a bare origin, a main checkout and a linked task worktree
# on a feature branch with an upstream, as commit steps require (#1528).
new_repo() {
    local dir="$1"
    mkdir -p "${dir}"
    git init -q --bare -b main "${dir}/origin.git"
    git init -q -b main "${dir}/main"
    configure_identity "${dir}/main"
    printf 'base\n' > "${dir}/main/README.md"
    git -C "${dir}/main" add README.md
    git -C "${dir}/main" commit -qm "seed"
    git -C "${dir}/main" remote add origin "${dir}/origin.git"
    git -C "${dir}/main" push -qu origin main 2>/dev/null
    git -C "${dir}/main" worktree add -q -b "feat/issue-1671-${dir##*/}" "${dir}/repo" 2>/dev/null
    git -C "${dir}/repo" push -qu origin "feat/issue-1671-${dir##*/}" 2>/dev/null
}

hooks_dir() {
    local d
    d="$(git -C "$1" rev-parse --git-path hooks)"
    case "${d}" in /*) ;; *) d="$1/${d}" ;; esac
    mkdir -p "${d}"
    printf '%s' "${d}"
}

# The pre-commit hook rejects staged content containing FORMAT_ME, in the
# pre-commit framework's output format. HOOK_OWN_ID overrides its id line
# (empty: none); HOOK_EXTRA_OUTPUT_FILE is appended to its output.
install_pre_commit_hook() {
    local hook
    hook="$(hooks_dir "$1")/pre-commit"
    cat > "${hook}" <<'HOOK'
#!/usr/bin/env bash
printf 'pre-commit SKIP=%s ALLOW_NO_CONFIG=%s\n' "${SKIP-unset}" "${PRE_COMMIT_ALLOW_NO_CONFIG-unset}" >> "${HOOK_LOG:-/dev/null}"
if git diff --cached -z --name-only --diff-filter=ACM | xargs -0 -r grep -l FORMAT_ME -- >/dev/null 2>&1; then
  echo "cargo fmt................................................................Failed"
  id="${HOOK_OWN_ID-cargo-fmt}"
  [ -z "$id" ] || echo "- hook id: $id"
  echo "- exit code: 1"
  [ -z "${HOOK_EXTRA_OUTPUT_FILE:-}" ] || cat "$HOOK_EXTRA_OUTPUT_FILE"
  exit 1
fi
exit 0
HOOK
    chmod +x "${hook}"
}

# The pre-push hook rejects a pushed tree containing PUSH_BAD.
install_pre_push_hook() {
    local hook
    hook="$(hooks_dir "$1")/pre-push"
    cat > "${hook}" <<'HOOK'
#!/usr/bin/env bash
printf 'pre-push\n' >> "${HOOK_LOG:-/dev/null}"
cat >/dev/null
if [ -n "${HOOK_PUSH_FORCE_FAIL:-}" ] || git grep -q PUSH_BAD HEAD -- 2>/dev/null; then
  echo "cargo test...............................................................Failed"
  echo "- hook id: ${HOOK_PUSH_ID:-cargo-test}"
  [ -z "${HOOK_EXTRA_OUTPUT_FILE:-}" ] || cat "$HOOK_EXTRA_OUTPUT_FILE"
  exit 1
fi
exit 0
HOOK
    chmod +x "${hook}"
}

# --- running the driver ---------------------------------------------------------
# run_driver <case-dir> <hook_repair_run args...>
# Runs hook_repair_run in a subshell at the top of the worktree (or in
# RUN_DRIVER_SUBDIR below it) under
# `set -euo pipefail`, as the step bodies do, and records:
#   rc, stdout, stderr, vars (HOOK_REPAIR_*), traps.changed, env.changed
# GIT_AUTHOR_* and GIT_COMMITTER_* are left out of env.changed: the commit
# helper exports them on purpose (amplihack_prepare_git_commit_identity).
run_driver() {
    local dir="$1"
    shift
    (
        set -euo pipefail
        cd "${dir}/repo${RUN_DRIVER_SUBDIR:+/${RUN_DRIVER_SUBDIR}}"
        export HOME="${dir}/home"
        mkdir -p "${HOME}"
        export STUB_STATE="${dir}/stub" AMPLIHACK_BIN="${STUB_BIN}/amplihack" PATH="${STUB_BIN}:${PATH}"
        export HOOK_LOG="${dir}/hook.log" TMPDIR="${TMPDIR_OVERRIDE:-${WORK}/tmp}"
        unset SKIP
        . "${GIT_IDENTITY}"; . "${CHECKPOINT_COMMIT}"
        . "${STAGE_HELPER}"; . "${DRIVER}"
        trap 'echo usr1' USR1
        traps_before="$(trap -p)"
        env_before="$(env | grep -E '^(SKIP=|PRE_COMMIT_|GIT_(DIR|WORK_TREE|INDEX_FILE|CONFIG|HOOKS)|HUSKY|LEFTHOOK)' | LC_ALL=C sort || true)"
        if hook_repair_run "$@" >"${dir}/stdout" 2>"${dir}/stderr"; then rc=0; else rc=$?; fi
        printf '%s\n' "${rc}" > "${dir}/rc"
        {
            printf 'STATUS=%s\n' "${HOOK_REPAIR_STATUS-__unset__}"
            printf 'IDS=%s\n' "${HOOK_REPAIR_HOOK_IDS-__unset__}"
            printf 'REASON=%s\n' "${HOOK_REPAIR_REASON-__unset__}"
            printf 'ROUNDS=%s\n' "${HOOK_REPAIR_ROUNDS-__unset__}"
            printf 'EVIDENCE_DIR=%s\n' "${HOOK_REPAIR_EVIDENCE_DIR-__unset__}"
        } > "${dir}/vars"
        [[ "$(trap -p)" == "${traps_before}" ]] || echo changed > "${dir}/traps.changed"
        env_after="$(env | grep -E '^(SKIP=|PRE_COMMIT_|GIT_(DIR|WORK_TREE|INDEX_FILE|CONFIG|HOOKS)|HUSKY|LEFTHOOK)' | LC_ALL=C sort || true)"
        [[ "${env_after}" == "${env_before}" ]] || printf '%s\n---\n%s\n' "${env_before}" "${env_after}" > "${dir}/env.changed"
    ) || { echo "harness: driver subshell failed in ${dir##*/}" > "${dir}/harness.err"; echo 99 > "${dir}/rc"; }
}
rc_of() { cat "$1/rc" 2>/dev/null || echo missing; }
var_of() { awk -v k="$2=" 'index($0, k) == 1 { print substr($0, length(k) + 1); exit }' "$1/vars" 2>/dev/null || true; }
result_line() { awk '/^\{"hook_repair_result":/ { l = $0 } END { if (l != "") print l }' "$1/stderr" 2>/dev/null || true; }
# result_field <case-dir> <python expression over r>
result_field() {
    result_line "$1" | python3 -I -c 'import json,sys
r=json.loads(sys.stdin.read())["hook_repair_result"]
v=eval(sys.argv[1])
print(",".join(v) if isinstance(v,list) else v)' "$2" 2>/dev/null || echo "__invalid__"
}

# Assertions every driver case shares: no stdout, no trap or hook-env change.
expect_driver_hygiene() {
    local label="$1" dir="$2"
    expect_empty_file "${label}: hook_repair_run writes nothing to stdout" "${dir}/stdout"
    [[ ! -e "${dir}/traps.changed" ]] && pass || fail "${label}: hook_repair_run changed the caller's traps"
    [[ ! -e "${dir}/env.changed" ]] && pass || fail "${label}: hook_repair_run changed SKIP/PRE_COMMIT_*/GIT_*/HUSKY/LEFTHOOK* in the caller"
    if [[ -f "${dir}/hook.log" ]] && grep -q 'SKIP=[^u]' "${dir}/hook.log"; then fail "${label}: a hook ran with SKIP set"; else pass; fi
    [[ -z "$(git -C "${dir}/repo" status --porcelain --untracked-files=all | grep -E 'round-[0-9]+\.evidence|hook-repair' || true)" ]] \
        && pass || fail "${label}: evidence was written inside the worktree"
    # The first attempt's output is kept in the git dir only while it runs.
    [[ -z "$(find "$(git -C "${dir}/repo" rev-parse --absolute-git-dir)" -maxdepth 1 -name 'hook-repair-attempt.*' -print)" ]] \
        && pass || fail "${label}: the first attempt's output was left in the git directory"
}

# commit_case <case-dir> [content]: a repo with the pre-commit hook and one
# staged file, ready for `hook_repair_run commit`.
commit_case() {
    local dir="$1" content="${2:-FORMAT_ME}"
    new_repo "${dir}"
    install_pre_commit_hook "${dir}/repo"
    printf '%s\n' "${content}" > "${dir}/repo/src.txt"
    git -C "${dir}/repo" add src.txt
}
COMMIT_ARGS=(commit checkpoint-after-implementation -- commit_with_pre_commit_guard -m "wip: checkpoint after implementation")

need() {
    local file="$1" section="$2"
    if [[ -f "${file}" ]]; then return 0; fi
    fail "section ${section}: ${file#"${REPO_ROOT}"/} does not exist"
    return 1
}

# =============================================================================
# A. stage_workflow_changes
# =============================================================================
section_a() {
    need "${STAGE_HELPER}" A || return 0
    local dir="${WORK}/a-stage" repo out err
    new_repo "${dir}"
    repo="${dir}/repo"
    printf 'tracked lock\n' > "${repo}/uv.lock"
    printf 'gone\n' > "${repo}/delete-me.txt"
    printf 'ignored/\n' > "${repo}/.gitignore"
    git -C "${repo}" add uv.lock delete-me.txt .gitignore
    git -C "${repo}" commit -qm "tracked files"
    printf 'changed\n' >> "${repo}/README.md"
    printf 'changed lock\n' >> "${repo}/uv.lock"
    rm "${repo}/delete-me.txt"
    mkdir -p "${repo}/src/deep" "${repo}/node_modules/pkg" "${repo}/web/node_modules/x" \
        "${repo}/crate/target/debug" "${repo}/py/__pycache__" "${repo}/py/pkg.egg-info" \
        "${repo}/sub" "${repo}/ignored" "${repo}/targets" "${repo}/my_node_modules" \
        "${repo}/target/release" "${repo}/src/target"
    printf 'new\n' > "${repo}/src/deep/new.rs"
    printf 'x\n' > "${repo}/node_modules/pkg/index.js"
    printf 'x\n' > "${repo}/web/node_modules/x/a.js"
    printf 'x\n' > "${repo}/crate/target/debug/bin"
    # A nested target/ is skipped only when Cargo marked it as a build directory.
    printf 'Signature: 8a477f597d28d172789f06886806bc55\n' > "${repo}/crate/target/CACHEDIR.TAG"
    printf 'x\n' > "${repo}/target/release/top"
    printf 'pub mod x;\n' > "${repo}/src/target/mod.rs"
    printf 'x\n' > "${repo}/py/__pycache__/m.pyc"
    printf 'x\n' > "${repo}/py/pkg.egg-info/PKG-INFO"
    # Lockfiles are source: an untracked one is staged at any depth.
    printf 'x\n' > "${repo}/sub/uv.lock"
    mkdir -p "${repo}/web"
    printf '{}\n' > "${repo}/web/package-lock.json"
    printf 'x\n' > "${repo}/Cargo.lock"
    printf 'x\n' > "${repo}/ignored/file"
    # Near misses that must be staged: the rule is on whole components.
    printf 'x\n' > "${repo}/targets/keep.txt"
    printf 'x\n' > "${repo}/my_node_modules/keep.txt"
    printf 'x\n' > "${repo}/uv.lock.bak"
    # A file named `*` must not act as a pathspec that stages everything (SR-5).
    printf 'star\n' > "${repo}/*"
    printf 'glob\n' > "${repo}/:(glob)x"
    # amplihack runtime state at the top is skipped; the same names deeper are not.
    mkdir -p "${repo}/.claude/runtime/metrics" "${repo}/.amplihack" "${repo}/docs/.amplihack" "${repo}/.claude/agents"
    printf 'x\n' > "${repo}/.claude/runtime/metrics/m.jsonl"
    printf 'x\n' > "${repo}/.amplihack/graph_db"
    printf 'x\n' > "${repo}/docs/.amplihack/keep.md"
    printf 'x\n' > "${repo}/.claude/agents/keep.md"

    out="${dir}/stage.out"; err="${dir}/stage.err"
    if ( cd "${repo}" && . "${STAGE_HELPER}" && stage_workflow_changes ) >"${out}" 2>"${err}"; then pass; else
        fail "A: stage_workflow_changes failed"; sed 's/^/    | /' "${err}" >&2
    fi
    expect_empty_file "A: stage_workflow_changes writes nothing to stdout" "${out}"
    local staged
    staged="$(git -C "${repo}" diff --cached --name-status --no-renames | LC_ALL=C sort)"
    printf '%s\n' "${staged}" > "${dir}/staged"
    expect_file_contains "A: tracked modification staged" "${dir}/staged" "M	README.md"
    expect_file_contains "A: tracked uv.lock modification staged" "${dir}/staged" "M	uv.lock"
    expect_file_contains "A: tracked deletion staged" "${dir}/staged" "D	delete-me.txt"
    expect_file_contains "A: new untracked file staged" "${dir}/staged" "A	src/deep/new.rs"
    expect_file_contains "A: targets/ is not target/" "${dir}/staged" "A	targets/keep.txt"
    expect_file_contains "A: a source directory named target is staged" "${dir}/staged" "A	src/target/mod.rs"
    expect_file_contains "A: my_node_modules/ is not node_modules/" "${dir}/staged" "A	my_node_modules/keep.txt"
    expect_file_contains "A: uv.lock.bak is staged" "${dir}/staged" "A	uv.lock.bak"
    expect_file_contains "A: an untracked nested uv.lock is staged" "${dir}/staged" "A	sub/uv.lock"
    expect_file_contains "A: an untracked package-lock.json is staged" "${dir}/staged" "A	web/package-lock.json"
    expect_file_contains "A: an untracked Cargo.lock is staged" "${dir}/staged" "A	Cargo.lock"
    expect_file_lacks "A: uv.lock is not reported as skipped" "${err}" "skipped sub/uv.lock"
    expect_file_contains "A: a file named * is staged as a name" "${dir}/staged" "A	*"
    expect_file_contains "A: a file named :(glob)x is staged as a name" "${dir}/staged" "A	:(glob)x"
    expect_file_contains "A: a nested .amplihack/ is staged" "${dir}/staged" "A	docs/.amplihack/keep.md"
    expect_file_contains "A: .claude/ outside runtime/ is staged" "${dir}/staged" "A	.claude/agents/keep.md"
    local excluded
    for excluded in node_modules/pkg/index.js web/node_modules/x/a.js crate/target/debug/bin \
        crate/target/CACHEDIR.TAG target/release/top \
        py/__pycache__/m.pyc py/pkg.egg-info/PKG-INFO \
        .claude/runtime/metrics/m.jsonl .amplihack/graph_db; do
        expect_file_lacks "A: ${excluded} is not staged" "${dir}/staged" "${excluded}"
        expect_file_contains "A: ${excluded} is reported on stderr" "${err}" "stage_workflow_changes: skipped ${excluded}"
    done
    expect_file_lacks "A: ignored files are not staged" "${dir}/staged" "ignored/file"

    local tree_before tree_after
    tree_before="$(git -C "${repo}" write-tree)"
    ( cd "${repo}" && . "${STAGE_HELPER}" && stage_workflow_changes ) >/dev/null 2>&1 || fail "A: second run failed"
    tree_after="$(git -C "${repo}" write-tree)"
    expect_eq "A: stage_workflow_changes is idempotent" "${tree_after}" "${tree_before}"
    local traps
    traps="$(cd "${repo}" && . "${STAGE_HELPER}" && trap 'echo t' USR1 && b="$(trap -p)" && stage_workflow_changes 2>/dev/null && [[ "$(trap -p)" == "${b}" ]] && echo same)"
    expect_eq "A: stage_workflow_changes installs no traps" "${traps}" "same"

    # Nothing to stage is success, not an error.
    local clean="${WORK}/a-clean"
    new_repo "${clean}"
    if ( cd "${clean}/repo" && . "${STAGE_HELPER}" && stage_workflow_changes ) >/dev/null 2>&1; then pass; else
        fail "A: stage_workflow_changes must succeed when nothing changed"
    fi

    # A repository with nothing tracked yet: `git add -u -- :/` failed here.
    local empty="${WORK}/a-empty"
    mkdir -p "${empty}"
    git init -q "${empty}/repo"
    printf 'first\n' > "${empty}/repo/first.txt"
    if ( cd "${empty}/repo" && . "${STAGE_HELPER}" && stage_workflow_changes ) >/dev/null 2>"${empty}/err"; then pass; else
        fail "A: stage_workflow_changes must succeed with nothing tracked"; sed 's/^/    | /' "${empty}/err" >&2
    fi
    expect_eq "A: first file staged in an empty repository" "$(git -C "${empty}/repo" diff --cached --name-only)" "first.txt"
}

# =============================================================================
# B. usage errors
# =============================================================================
section_b() {
    need "${DRIVER}" B || return 0
    local i=0 args
    local -a cases=(
        "deploy checkpoint-after-implementation -- true"
        "commit Checkpoint -- true"
        "commit a;b -- true"
        "commit $(printf 'a%.0s' {1..65}) -- true"
        "commit checkpoint-after-implementation true"
        "commit checkpoint-after-implementation --"
        "push step-15-commit-push -- git push"
        "commit checkpoint-after-implementation --fix-message fix -- true"
    )
    for args in "${cases[@]}"; do
        i=$((i + 1))
        local dir="${WORK}/b-usage-${i}"
        new_repo "${dir}"
        # shellcheck disable=SC2086
        run_driver "${dir}" ${args}
        expect_eq "B${i}: usage error for '${args}' returns 2" "$(rc_of "${dir}")" "2"
        expect_eq "B${i}: usage status" "$(var_of "${dir}" STATUS)" "usage"
        expect_eq "B${i}: no recipe ran" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"
    done
    # --fix-message must be one line with no control characters.
    local dir="${WORK}/b-usage-multiline"
    new_repo "${dir}"
    run_driver "${dir}" push step-15-commit-push --fix-message "$(printf 'fix: one\nfix: two')" -- git push
    expect_eq "B: multi-line --fix-message is a usage error" "$(rc_of "${dir}")" "2"
    dir="${WORK}/b-usage-ctrl"
    new_repo "${dir}"
    run_driver "${dir}" push step-15-commit-push --fix-message "$(printf 'fix: \001 bell')" -- git push
    expect_eq "B: --fix-message with a control character is a usage error" "$(rc_of "${dir}")" "2"

    # HR_DIR inside the worktree is a usage error, and no agent runs (SR-14).
    dir="${WORK}/b-hrdir"
    commit_case "${dir}"
    mkdir -p "${dir}/repo/.tmp-inside"
    TMPDIR_OVERRIDE="${dir}/repo/.tmp-inside" run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "B: HR_DIR inside the worktree returns 2" "$(rc_of "${dir}")" "2"
    expect_eq "B: HR_DIR inside the worktree runs no agent" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"
    expect_eq "B: HR_DIR inside the worktree is checked after the first attempt" "$(hook_calls "${dir}/hook.log" pre-commit)" "1"
    [[ -z "$(ls -A "${dir}/repo/.tmp-inside")" ]] && pass || fail "B: nothing may be left in a TMPDIR inside the worktree"

    # A first-try success creates nothing under TMPDIR, so a TMPDIR inside the
    # worktree, missing, or read-only changes nothing (item 4 of the review).
    local tmp_case tmp_dir
    for tmp_case in inside missing readonly; do
        dir="${WORK}/b-first-try-${tmp_case}"
        commit_case "${dir}" "clean content"
        case "${tmp_case}" in
            inside) tmp_dir="${dir}/repo/.tmp-inside"; mkdir -p "${tmp_dir}" ;;
            missing) tmp_dir="${dir}/no-such-tmp" ;;
            readonly) tmp_dir="${dir}/ro-tmp"; mkdir -p "${tmp_dir}"; chmod 0500 "${tmp_dir}" ;;
        esac
        TMPDIR_OVERRIDE="${tmp_dir}" run_driver "${dir}" "${COMMIT_ARGS[@]}"
        expect_eq "B first try, TMPDIR ${tmp_case}: rc 0" "$(rc_of "${dir}")" "0"
        expect_eq "B first try, TMPDIR ${tmp_case}: status ok" "$(var_of "${dir}" STATUS)" "ok"
        expect_eq "B first try, TMPDIR ${tmp_case}: committed" "$(git -C "${dir}/repo" log -1 --format=%s)" "wip: checkpoint after implementation"
        expect_driver_hygiene "B first try, TMPDIR ${tmp_case}" "${dir}"
        [[ "${tmp_case}" != inside ]] || { [[ -z "$(ls -A "${dir}/repo/.tmp-inside")" ]] && pass || fail "B first try: nothing written to TMPDIR"; }
        [[ "${tmp_case}" != readonly ]] || chmod 0700 "${dir}/ro-tmp"
    done
}

# =============================================================================
# C. classification and hook ids
# =============================================================================
section_c() {
    need "${DRIVER}" C || return 0
    local dir

    # C1 success: ok, rc 0, no rounds, nothing kept, no nested recipe at all.
    dir="${WORK}/c1-ok"
    commit_case "${dir}" "clean content"
    run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "C1: success returns 0" "$(rc_of "${dir}")" "0"
    expect_eq "C1: status ok" "$(var_of "${dir}" STATUS)" "ok"
    expect_eq "C1: rounds 0" "$(var_of "${dir}" ROUNDS)" "0"
    expect_eq "C1: no evidence dir kept" "$(var_of "${dir}" EVIDENCE_DIR)" ""
    expect_eq "C1: no recipe ran" "$(cat "${dir}/stub/calls" 2>/dev/null | wc -l | tr -d ' ')" "0"
    expect_eq "C1: the commit was made" "$(git -C "${dir}/repo" log -1 --format=%s)" "wip: checkpoint after implementation"
    expect_driver_hygiene C1 "${dir}"

    # C2 nothing staged: not a hook rejection; the command's rc is returned.
    dir="${WORK}/c2-nothing-staged"
    new_repo "${dir}"
    install_pre_commit_hook "${dir}/repo"
    run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "C2: nothing staged is not_hook" "$(var_of "${dir}" STATUS)" "not_hook"
    expect_eq "C2: rc is git commit's rc" "$(rc_of "${dir}")" "1"
    expect_eq "C2: no agent ran" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"
    [[ ! -s "${dir}/stub/show" ]] && pass || fail "C2: a non-hook failure must not even resolve the evaluator"
    expect_driver_hygiene C2 "${dir}"

    # C3 staged, no hook, command fails: not_hook with the command's own rc.
    dir="${WORK}/c3-no-hook"
    new_repo "${dir}"
    printf 'x\n' > "${dir}/repo/src.txt"
    git -C "${dir}/repo" add src.txt
    run_driver "${dir}" commit checkpoint-after-implementation -- sh -c 'echo boom >&2; exit 3'
    expect_eq "C3: no hook is not_hook" "$(var_of "${dir}" STATUS)" "not_hook"
    expect_eq "C3: rc is the command's rc" "$(rc_of "${dir}")" "3"
    expect_eq "C3: ids unknown" "$(var_of "${dir}" IDS)" "unknown"

    # C4 a hook file that is not executable does not count.
    dir="${WORK}/c4-nonexec"
    commit_case "${dir}"
    chmod -x "$(hooks_dir "${dir}/repo")/pre-commit"
    run_driver "${dir}" commit checkpoint-after-implementation -- sh -c 'exit 1'
    expect_eq "C4: non-executable hook is not_hook" "$(var_of "${dir}" STATUS)" "not_hook"

    # C5 a commit-msg hook counts as well.
    dir="${WORK}/c5-commit-msg"
    new_repo "${dir}"
    printf '#!/usr/bin/env bash\necho "- hook id: commit-msg-lint"\nexit 1\n' > "$(hooks_dir "${dir}/repo")/commit-msg"
    chmod +x "$(hooks_dir "${dir}/repo")/commit-msg"
    printf 'x\n' > "${dir}/repo/src.txt"; git -C "${dir}/repo" add src.txt
    STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "C5: commit-msg rejection is repaired" "$(var_of "${dir}" STATUS)" "stuck"
    expect_eq "C5: commit-msg hook id parsed" "$(var_of "${dir}" IDS)" "commit-msg-lint"

    # C6 core.hooksPath is honoured, read only.
    dir="${WORK}/c6-hookspath"
    new_repo "${dir}"
    mkdir -p "${dir}/custom-hooks"
    git -C "${dir}/repo" config core.hooksPath "${dir}/custom-hooks"
    install_pre_commit_hook "${dir}/repo"
    [[ -x "${dir}/custom-hooks/pre-commit" ]] || fail "C6: harness did not install into core.hooksPath"
    printf 'FORMAT_ME\n' > "${dir}/repo/src.txt"; git -C "${dir}/repo" add src.txt
    STUB_AGENT_ACTION=fix run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "C6: hook under core.hooksPath is repaired" "$(var_of "${dir}" STATUS)" "ok"
    expect_eq "C6: core.hooksPath unchanged" "$(git -C "${dir}/repo" config --get core.hooksPath)" "${dir}/custom-hooks"

    # Hook ids: sorted, de-duplicated, whitelisted, at most 20, else unknown.
    dir="${WORK}/c7-ids"
    commit_case "${dir}"
    printf -- '- hook id: zeta\n- hook id: alpha\n- hook id: alpha\n- hook id: bad;rm\n- hook id: a&b\n- hook id: %s\n' \
        "$(printf 'x%.0s' {1..65})" > "${dir}/extra"
    HOOK_EXTRA_OUTPUT_FILE="${dir}/extra" STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "C7: ids sorted, de-duplicated, invalid dropped" "$(var_of "${dir}" IDS)" "alpha,cargo-fmt,zeta"

    dir="${WORK}/c8-ids-none"
    commit_case "${dir}"
    printf -- '- hook id: has space\n- hook id: $(rm -rf x)\n' > "${dir}/extra"
    HOOK_OWN_ID="" HOOK_EXTRA_OUTPUT_FILE="${dir}/extra" STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "C8: no valid id gives unknown" "$(var_of "${dir}" IDS)" "unknown"
    expect_eq "C8: status still stuck (unknown ids still repaired)" "$(var_of "${dir}" STATUS)" "stuck"

    dir="${WORK}/c9-ids-cap"
    commit_case "${dir}"
    : > "${dir}/extra"
    local n
    for n in $(seq -w 1 25); do printf -- '- hook id: id%s\n' "${n}" >> "${dir}/extra"; done
    HOOK_OWN_ID="" HOOK_EXTRA_OUTPUT_FILE="${dir}/extra" STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "C9: at most 20 ids kept" "$(var_of "${dir}" IDS | tr ',' '\n' | grep -c .)" "20"
    expect_eq "C9: the first 20 in sort order" "$(var_of "${dir}" IDS | awk -F, '{ print $NF }')" "id20"

    # C10 git's own fatal: line is not a hook rejection, even with a hook present.
    dir="${WORK}/c10-fatal"
    commit_case "${dir}"
    run_driver "${dir}" commit checkpoint-after-implementation -- sh -c 'echo "fatal: unable to write new index file" >&2; exit 128'
    expect_eq "C10: fatal: is not_hook" "$(var_of "${dir}" STATUS)" "not_hook"
    expect_eq "C10: rc is the command's" "$(rc_of "${dir}")" "128"
    expect_eq "C10: no agent ran" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"

    # C11 a signing failure after the hook passed is not a hook rejection.
    dir="${WORK}/c11-gpg"
    commit_case "${dir}" "clean content"
    run_driver "${dir}" commit checkpoint-after-implementation -- \
        git -c commit.gpgsign=true -c gpg.program=false commit -m "wip: signed"
    expect_eq "C11: gpg failure is not_hook" "$(var_of "${dir}" STATUS)" "not_hook"
    expect_ne "C11: rc non-zero" "$(rc_of "${dir}")" "0"
    expect_eq "C11: no agent ran" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"
    expect_driver_hygiene C11 "${dir}"

    # C12 the gpg message alone, from a hook that also failed, is not repaired.
    dir="${WORK}/c12-gpg-text"
    commit_case "${dir}"
    printf 'error: gpg failed to sign the data\n' > "${dir}/extra"
    HOOK_EXTRA_OUTPUT_FILE="${dir}/extra" run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "C12: gpg text is not_hook" "$(var_of "${dir}" STATUS)" "not_hook"

    # C13 a fatal: line inside the hook's own output, not at the end, is still
    # a hook rejection: only the last non-empty line is git's own.
    dir="${WORK}/c13-fatal-mid-hook"
    commit_case "${dir}"
    printf 'fatal: from the hook test run\n\ntest result: FAILED\n' > "${dir}/extra"
    HOOK_EXTRA_OUTPUT_FILE="${dir}/extra" STUB_AGENT_ACTION=fix run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "C13: fatal: mid hook output is repaired" "$(var_of "${dir}" STATUS)" "ok"
    expect_eq "C13: one agent round" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "1"
}

# Push classification lives with the push cases in section G.

# =============================================================================
# D. the loop
# =============================================================================
section_d() {
    need "${DRIVER}" D || return 0
    local dir hr

    # D1 fixed in one round: ok, no evaluator call, same message, one commit.
    dir="${WORK}/d1-fixed"
    commit_case "${dir}"
    printf 'UNIQUE_HOOK_TEXT_42\n' > "${dir}/extra"
    local seed
    seed="$(git -C "${dir}/repo" rev-parse HEAD)"
    HOOK_EXTRA_OUTPUT_FILE="${dir}/extra" STUB_AGENT_ACTION=fix run_driver "${dir}" \
        commit checkpoint-after-implementation --output "${dir}/final.out" -- commit_with_pre_commit_guard -m "wip: checkpoint after implementation"
    expect_eq "D1: rc 0" "$(rc_of "${dir}")" "0"
    expect_eq "D1: status ok" "$(var_of "${dir}" STATUS)" "ok"
    expect_eq "D1: rounds 1" "$(var_of "${dir}" ROUNDS)" "1"
    expect_eq "D1: ids from the rejection" "$(var_of "${dir}" IDS)" "cargo-fmt"
    expect_eq "D1: evaluator not called when the retry succeeds" "$(stub_calls "${dir}/stub" loop-health-evaluator)" "0"
    expect_eq "D1: the retry used the same message" "$(git -C "${dir}/repo" log -1 --format=%s)" "wip: checkpoint after implementation"
    expect_eq "D1: exactly one new commit" "$(git -C "${dir}/repo" rev-parse HEAD~1 2>/dev/null)" "${seed}"
    expect_eq "D1: the committed content is the agent's fix" "$(git -C "${dir}/repo" show HEAD:src.txt 2>/dev/null)" "formatted"
    expect_eq "D1: evidence dir removed on ok" "$(var_of "${dir}" EVIDENCE_DIR)" ""
    expect_file_lacks "D1: --output holds the final (successful) attempt" "${dir}/final.out" "Failed"
    [[ -f "${dir}/final.out" ]] && pass || fail "D1: --output file was not written"
    expect_driver_hygiene D1 "${dir}"

    # The fixing recipe: default path from the driver's bundle, its inputs, and
    # no hook output passed by argument (SR-10).
    expect_eq "D1: fixing recipe resolved beside the driver" "$(cat "${dir}/stub/hook-rejection-repair.1.path" 2>/dev/null)" \
        "${RECIPES}/hook-rejection-repair.yaml"
    expect_eq "D1: hook_repair_worktree" "$(stub_arg "${dir}/stub" hook-rejection-repair 1 hook_repair_worktree)" "${dir}/repo"
    expect_eq "D1: hook_repair_operation" "$(stub_arg "${dir}/stub" hook-rejection-repair 1 hook_repair_operation)" "commit"
    expect_eq "D1: hook_repair_step" "$(stub_arg "${dir}/stub" hook-rejection-repair 1 hook_repair_step)" "checkpoint-after-implementation"
    expect_eq "D1: hook_repair_hook_ids" "$(stub_arg "${dir}/stub" hook-rejection-repair 1 hook_repair_hook_ids)" "cargo-fmt"
    local ev
    ev="$(stub_arg "${dir}/stub" hook-rejection-repair 1 hook_repair_evidence_file || true)"
    case "${ev}" in "${dir}/repo"/*|"") fail "D1: evidence file must exist outside the worktree (got '${ev}')" ;; *) pass ;; esac
    expect_eq "D1: evidence file is round-1.evidence" "${ev##*/}" "round-1.evidence"
    [[ -f "${dir}/stub/evidence.1" ]] && pass || fail "D1: the evidence file did not exist when the agent ran"
    expect_file_contains "D1: evidence holds the hook output" "${dir}/stub/evidence.1" "UNIQUE_HOOK_TEXT_42"
    expect_file_contains "D1: evidence names the operation" "${dir}/stub/evidence.1" "commit"
    expect_file_contains "D1: evidence names the step" "${dir}/stub/evidence.1" "checkpoint-after-implementation"
    expect_file_contains "D1: evidence lists staged files" "${dir}/stub/evidence.1" "src.txt"
    expect_file_contains "D1: evidence holds the hook ids" "${dir}/stub/evidence.1" "cargo-fmt"
    : > "${dir}/agent.argv"
    [[ ! -f "${dir}/stub/hook-rejection-repair.1.argv" ]] || tr '\0' '\n' < "${dir}/stub/hook-rejection-repair.1.argv" > "${dir}/agent.argv"
    if grep -qF UNIQUE_HOOK_TEXT_42 "${dir}/agent.argv"; then
        fail "D1: hook output must reach the agent only through the evidence file, not argv"
    else pass; fi
    if grep -q '^hook_repair_output_tail=' "${dir}/agent.argv"; then
        fail "D1: hook_repair_output_tail must not be passed until SR-10 is confirmed"
    else pass; fi
    hr="$(dirname "${ev}")"
    [[ ! -e "${hr}" ]] && pass || fail "D1: HR_DIR ${hr} must be deleted on ok"

    # D2 STUCK: rc 1, evaluator inputs, result line, evidence kept (mode 700).
    dir="${WORK}/d2-stuck"
    commit_case "${dir}"
    seed="$(git -C "${dir}/repo" rev-parse HEAD)"
    STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "D2: rc 1" "$(rc_of "${dir}")" "1"
    expect_eq "D2: status stuck" "$(var_of "${dir}" STATUS)" "stuck"
    expect_eq "D2: rounds 1" "$(var_of "${dir}" ROUNDS)" "1"
    expect_eq "D2: evaluator called once" "$(stub_calls "${dir}/stub" loop-health-evaluator)" "1"
    expect_eq "D2: HEAD unchanged" "$(git -C "${dir}/repo" rev-parse HEAD)" "${seed}"
    expect_eq "D2: first attempt plus one retry" "$(hook_calls "${dir}/hook.log" pre-commit)" "2"
    hr="$(var_of "${dir}" EVIDENCE_DIR)"
    [[ -n "${hr}" && -d "${hr}" ]] && pass || fail "D2: evidence dir must be kept on stuck (got '${hr}')"
    [[ -f "${hr}/round-1.evidence" ]] && pass || fail "D2: round-1.evidence kept"
    [[ "$(ls -ld "${hr}" 2>/dev/null | cut -c1-10)" == "drwx------" ]] && pass || fail "D2: HR_DIR must be mode 700"
    case "${hr}" in "${dir}/repo"/*) fail "D2: HR_DIR inside the worktree" ;; *) pass ;; esac
    expect_eq "D2: evaluator resolved beside the driver" "$(cat "${dir}/stub/loop-health-evaluator.1.path" 2>/dev/null)" \
        "${RECIPES}/loop-health-evaluator.yaml"
    expect_file_contains "D2: evaluator checked with recipe show first" "${dir}/stub/show" "${RECIPES}/loop-health-evaluator.yaml"
    local s="${dir}/stub"
    expect_eq "D2: loop_name" "$(stub_arg "${s}" loop-health-evaluator 1 loop_name)" "hook-repair:checkpoint-after-implementation:commit"
    expect_eq "D2: loop_round_label" "$(stub_arg "${s}" loop-health-evaluator 1 loop_round_label)" "1"
    expect_eq "D2: loop_repo_path" "$(stub_arg "${s}" loop-health-evaluator 1 loop_repo_path)" "${dir}/repo"
    expect_eq "D2: loop_agent_working_dir is HR_DIR" "$(stub_arg "${s}" loop-health-evaluator 1 loop_agent_working_dir)" "${hr}"
    expect_eq "D2: loop_health_enforce" "$(stub_arg "${s}" loop-health-evaluator 1 loop_health_enforce)" "false"
    expect_eq "D2: loop_clean_token" "$(stub_arg "${s}" loop-health-evaluator 1 loop_clean_token)" "HOOK_PASSED"
    expect_eq "D2: loop_round_verdict" "$(stub_arg "${s}" loop-health-evaluator 1 loop_round_verdict)" "fixed"
    expect_eq "D2: loop_findings_current" "$(stub_arg "${s}" loop-health-evaluator 1 loop_findings_current)" "cargo-fmt"
    expect_eq "D2: loop_baseline_ref" "$(stub_arg "${s}" loop-health-evaluator 1 loop_baseline_ref)" "${seed}"
    expect_eq "D2: loop_child_exit_code" "$(stub_arg "${s}" loop-health-evaluator 1 loop_child_exit_code)" "1"
    local tail_out
    tail_out="$(stub_arg "${s}" loop-health-evaluator 1 loop_last_round_output || true)"
    [[ "${tail_out}" == *"cargo-fmt"* ]] && pass || fail "D2: loop_last_round_output holds the retry's hook output"
    [[ "${#tail_out}" -le 16384 ]] && pass || fail "D2: loop_last_round_output is capped at 16 KiB"
    [[ -n "$(stub_arg "${s}" loop-health-evaluator 1 loop_history || true)" ]] && pass || fail "D2: loop_history is passed"
    # SR-10 / #1675: the hook output and history never appear in argv.
    expect_file_lacks "D2: evaluator argv holds no hook output" "${s}/loop-health-evaluator.1.argv" "hook id:"
    expect_file_lacks "D2: evaluator argv holds no history" "${s}/loop-health-evaluator.1.argv" "output_sha256="
    [[ "$(cat "${s}/loop-health-evaluator.1.cwd" 2>/dev/null)" != "${dir}/repo" ]] && pass \
        || fail "D2: the evaluator must not start in the worktree (#1528)"
    expect_eq "D2: result status" "$(result_field "${dir}" 'r["status"]')" "stuck"
    expect_eq "D2: result version" "$(result_field "${dir}" 'r["version"]')" "1"
    expect_eq "D2: result operation" "$(result_field "${dir}" 'r["operation"]')" "commit"
    expect_eq "D2: result step" "$(result_field "${dir}" 'r["step"]')" "checkpoint-after-implementation"
    expect_eq "D2: result hook_ids" "$(result_field "${dir}" 'r["hook_ids"]')" "cargo-fmt"
    expect_eq "D2: result loop_verdict" "$(result_field "${dir}" 'r["loop_verdict"]')" "STUCK"
    expect_eq "D2: result rounds" "$(result_field "${dir}" 'r["rounds"]')" "1"
    expect_eq "D2: result evidence_dir" "$(result_field "${dir}" 'r["evidence_dir"]')" "${hr}"
    expect_file_contains "D2: hook output printed on stderr" "${dir}/stderr" "cargo fmt"
    expect_file_lacks "D2: agent summary never in stderr result" "${dir}/stderr" "AGENT_SUMMARY_SENTINEL"
    expect_driver_hygiene D2 "${dir}"

    # D3 CONTINUE then fixed: two rounds, one evaluator call, history grows.
    dir="${WORK}/d3-continue"
    commit_case "${dir}"
    STUB_AGENT_ACTION="noop fix" STUB_VERDICTS="CONTINUE" run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "D3: rc 0" "$(rc_of "${dir}")" "0"
    expect_eq "D3: rounds 2" "$(var_of "${dir}" ROUNDS)" "2"
    expect_eq "D3: evaluator called once" "$(stub_calls "${dir}/stub" loop-health-evaluator)" "1"
    expect_eq "D3: second evidence file is round-2" \
        "$(basename "$(stub_arg "${dir}/stub" hook-rejection-repair 2 hook_repair_evidence_file || true)")" "round-2.evidence"

    # D4 CONTINUE, CONTINUE, STUCK: history has a line per round.
    dir="${WORK}/d4-history"
    commit_case "${dir}"
    STUB_AGENT_ACTION=noop STUB_VERDICTS="CONTINUE CONTINUE STUCK" run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "D4: rounds 3" "$(var_of "${dir}" ROUNDS)" "3"
    expect_eq "D4: status stuck" "$(var_of "${dir}" STATUS)" "stuck"
    expect_eq "D4: loop_round_label 3" "$(stub_arg "${dir}/stub" loop-health-evaluator 3 loop_round_label)" "3"
    expect_eq "D4: loop_history has one line per round" \
        "$(stub_arg "${dir}/stub" loop-health-evaluator 3 loop_history | grep -c .)" "3"
    expect_eq "D4: findings_previous carries the last ids" \
        "$(stub_arg "${dir}/stub" loop-health-evaluator 2 loop_findings_previous)" "cargo-fmt"

    # D5 DONE while still failing.
    dir="${WORK}/d5-done"
    commit_case "${dir}"
    STUB_AGENT_ACTION=noop STUB_VERDICTS=DONE run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "D5: rc 1" "$(rc_of "${dir}")" "1"
    expect_eq "D5: status evaluator_done_unresolved" "$(var_of "${dir}" STATUS)" "evaluator_done_unresolved"
    [[ "$(var_of "${dir}" REASON)" == *"evaluator ended the loop while the hook still rejects"* ]] && pass \
        || fail "D5: reason must say the evaluator ended the loop while the hook still rejects (got '$(var_of "${dir}" REASON)')"
    expect_eq "D5: result loop_verdict" "$(result_field "${dir}" 'r["loop_verdict"]')" "DONE"

    # D6 no usable verdict counts as STUCK.
    local v
    for v in MALFORMED NONE PLANTED RC3; do
        dir="${WORK}/d6-$(printf '%s' "$v" | tr '[:upper:]' '[:lower:]')"
        commit_case "${dir}"
        STUB_AGENT_ACTION=noop STUB_VERDICTS="${v}" run_driver "${dir}" "${COMMIT_ARGS[@]}"
        expect_eq "D6 ${v}: rc 1" "$(rc_of "${dir}")" "1"
        expect_eq "D6 ${v}: counts as stuck" "$(var_of "${dir}" STATUS)" "stuck"
        expect_eq "D6 ${v}: no second round" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "1"
    done

    # D7 agent status from a closed set; class and summary stay in evidence.
    dir="${WORK}/d7-unreported"
    commit_case "${dir}"
    STUB_AGENT_STATUS='fixed"; rm -rf /' STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "D7: unknown agent status is unreported" \
        "$(stub_arg "${dir}/stub" loop-health-evaluator 1 loop_round_verdict)" "unreported"
    dir="${WORK}/d7-none"
    commit_case "${dir}"
    STUB_AGENT_STATUS=none STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "D7: missing agent line is unreported" \
        "$(stub_arg "${dir}/stub" loop-health-evaluator 1 loop_round_verdict)" "unreported"

    # D8 environment-class rejection: not_fixable_in_code, no change and the
    # same hook output ends the loop as stuck without asking the evaluator.
    dir="${WORK}/d8-env"
    commit_case "${dir}"
    STUB_AGENT_STATUS=not_fixable_in_code STUB_AGENT_CLASS=environment STUB_AGENT_ACTION=noop STUB_VERDICTS=CONTINUE \
        run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "D8: rc 1" "$(rc_of "${dir}")" "1"
    expect_eq "D8: status stuck" "$(var_of "${dir}" STATUS)" "stuck"
    expect_eq "D8: the retry still ran" "$(hook_calls "${dir}/hook.log" pre-commit)" "2"
    expect_eq "D8: evaluator not called" "$(stub_calls "${dir}/stub" loop-health-evaluator)" "0"
    expect_eq "D8: one round" "$(var_of "${dir}" ROUNDS)" "1"
    [[ "$(var_of "${dir}" REASON)" == "no progress: "* ]] && pass || fail "D8: reason must start 'no progress: ' (got '$(var_of "${dir}" REASON)')"
    expect_eq "D8: result loop_verdict empty" "$(result_field "${dir}" 'r["loop_verdict"]')" ""
    expect_file_contains "D8: history records the round" "$(var_of "${dir}" EVIDENCE_DIR)/history" "output_changed=no agent=not_fixable_in_code diff=no diff"

    # D8b not_fixable_in_code whose hook output changes still goes to the evaluator.
    dir="${WORK}/d8b-env-output-changes"
    commit_case "${dir}"
    : > "${dir}/extra"
    HOOK_EXTRA_OUTPUT_FILE="${dir}/extra" STUB_AGENT_BODY="echo another line >> '${dir}/extra'" \
        STUB_AGENT_STATUS=not_fixable_in_code STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "D8b: changed output reaches the evaluator" \
        "$(stub_arg "${dir}/stub" loop-health-evaluator 1 loop_round_verdict)" "not_fixable_in_code"
    expect_eq "D8b: status stuck from the evaluator" "$(result_field "${dir}" 'r["loop_verdict"]')" "STUCK"

    # D8c not_fixable_in_code with a staged change still goes to the evaluator.
    dir="${WORK}/d8c-env-tree-changes"
    commit_case "${dir}"
    STUB_AGENT_BODY="echo note > notes.txt" \
        STUB_AGENT_STATUS=not_fixable_in_code STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "D8c: a changed tree reaches the evaluator" "$(stub_calls "${dir}/stub" loop-health-evaluator)" "1"

    # D8d an agent that reports fixed but changed nothing still goes to the evaluator.
    dir="${WORK}/d8d-fixed-no-change"
    commit_case "${dir}"
    STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "D8d: fixed with no change reaches the evaluator" "$(stub_calls "${dir}/stub" loop-health-evaluator)" "1"

    # D9 terminal 79 from the agent, by rc or by marker, and from the evaluator.
    local mode
    for mode in agent-rc agent-marker evaluator-rc; do
        dir="${WORK}/d9-${mode}"
        commit_case "${dir}"
        case "${mode}" in
            agent-rc) STUB_AGENT_RC=79 run_driver "${dir}" "${COMMIT_ARGS[@]}" ;;
            agent-marker) STUB_AGENT_TERMINAL=1 run_driver "${dir}" "${COMMIT_ARGS[@]}" ;;
            evaluator-rc) STUB_AGENT_ACTION=noop STUB_VERDICTS=RC79 run_driver "${dir}" "${COMMIT_ARGS[@]}" ;;
        esac
        expect_eq "D9 ${mode}: rc 79" "$(rc_of "${dir}")" "79"
        expect_eq "D9 ${mode}: status terminal" "$(var_of "${dir}" STATUS)" "terminal"
        expect_eq "D9 ${mode}: no result line" "$(result_line "${dir}")" ""
        hr="$(var_of "${dir}" EVIDENCE_DIR)"
        [[ -n "${hr}" && -d "${hr}" ]] && pass || fail "D9 ${mode}: evidence dir reported in HOOK_REPAIR_EVIDENCE_DIR"
        expect_eq "D9 ${mode}: one agent round only" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "1"
    done
    expect_eq "D9 agent-rc: no retry after 79" "$(hook_calls "${WORK}/d9-agent-rc/hook.log" pre-commit)" "1"
    expect_eq "D9 agent-rc: no evaluator after 79" "$(stub_calls "${WORK}/d9-agent-rc/stub" loop-health-evaluator)" "0"

    # D10 an evaluator that does not resolve: stuck before any agent runs.
    dir="${WORK}/d10-unresolved"
    commit_case "${dir}"
    STUB_HEALTH_RESOLVES=1 run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "D10: rc 1" "$(rc_of "${dir}")" "1"
    expect_eq "D10: status stuck" "$(var_of "${dir}" STATUS)" "stuck"
    expect_eq "D10: rounds 0" "$(var_of "${dir}" ROUNDS)" "0"
    expect_eq "D10: reason" "$(var_of "${dir}" REASON)" "loop-health-evaluator does not resolve"
    expect_eq "D10: no agent ran" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"

    # D11 the recipe overrides are honoured.
    dir="${WORK}/d11-overrides"
    commit_case "${dir}"
    mkdir -p "${dir}/alt"
    : > "${dir}/alt/hook-rejection-repair.yaml"; : > "${dir}/alt/loop-health-evaluator.yaml"
    HOOK_REPAIR_ROUND_RECIPE="${dir}/alt/hook-rejection-repair.yaml" \
    HOOK_REPAIR_HEALTH_RECIPE="${dir}/alt/loop-health-evaluator.yaml" \
        STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "D11: HOOK_REPAIR_ROUND_RECIPE" "$(cat "${dir}/stub/hook-rejection-repair.1.path" 2>/dev/null)" "${dir}/alt/hook-rejection-repair.yaml"
    expect_eq "D11: HOOK_REPAIR_HEALTH_RECIPE" "$(cat "${dir}/stub/loop-health-evaluator.1.path" 2>/dev/null)" "${dir}/alt/loop-health-evaluator.yaml"

    # D12 the kept evidence directory holds no raw agent or evaluator log.
    local tok="gh""p_""BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"
    dir="${WORK}/d12-redacted-logs"
    commit_case "${dir}"
    STUB_AGENT_SUMMARY="pushed with ${tok}" STUB_HEALTH_REASON="saw ${tok} in the output" STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK \
        run_driver "${dir}" "${COMMIT_ARGS[@]}"
    hr="$(var_of "${dir}" EVIDENCE_DIR)"
    [[ -f "${hr}/agent-1.log" && -f "${hr}/health-1.log" ]] && pass || fail "D12: the redacted agent and evaluator logs are kept"
    if grep -rqF -- "${tok}" "${hr}" 2>/dev/null; then fail "D12: a raw token remains in ${hr##*/}"; else pass; fi
    expect_eq "D12: no raw attempt file" "$(find "${hr}" -name '*.raw' -o -name '*.redacted' | wc -l | tr -d ' ')" "0"
    expect_file_contains "D12: the agent log is kept, redacted" "${hr}/agent-1.log" "agent: working on the hook rejection"
    dir="${WORK}/d12-redacted-terminal"
    commit_case "${dir}"
    STUB_AGENT_SUMMARY="pushed with ${tok}" STUB_AGENT_RC=79 run_driver "${dir}" "${COMMIT_ARGS[@]}"
    if grep -rqF -- "${tok}" "$(var_of "${dir}" EVIDENCE_DIR)" 2>/dev/null; then fail "D12 terminal: a raw token remains"; else pass; fi
}

# =============================================================================
# E. refusal rules
# =============================================================================
section_e() {
    need "${DRIVER}" E || return 0
    local dir

    dir="${WORK}/e1-pre-commit-config"
    commit_case "${dir}"
    STUB_AGENT_ACTION=edit-pre-commit-config STUB_VERDICTS=CONTINUE run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "E1: rc 1" "$(rc_of "${dir}")" "1"
    expect_eq "E1: status refused" "$(var_of "${dir}" STATUS)" "refused"
    [[ "$(var_of "${dir}" REASON)" == "hook-config-modified: "*".pre-commit-config.yaml"* ]] && pass \
        || fail "E1: reason must be hook-config-modified naming .pre-commit-config.yaml (got '$(var_of "${dir}" REASON)')"
    expect_eq "E1: never retried" "$(hook_calls "${dir}/hook.log" pre-commit)" "1"
    expect_eq "E1: evaluator not called" "$(stub_calls "${dir}/stub" loop-health-evaluator)" "0"
    expect_eq "E1: result status" "$(result_field "${dir}" 'r["status"]')" "refused"
    expect_eq "E1: result loop_verdict empty" "$(result_field "${dir}" 'r["loop_verdict"]')" ""
    [[ -n "$(var_of "${dir}" EVIDENCE_DIR)" ]] && pass || fail "E1: evidence kept on refused"

    dir="${WORK}/e2-move-head"
    commit_case "${dir}"
    STUB_AGENT_ACTION=move-head STUB_VERDICTS=CONTINUE run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "E2: status refused" "$(var_of "${dir}" STATUS)" "refused"
    expect_eq "E2: reason agent-moved-head" "$(var_of "${dir}" REASON)" "agent-moved-head"
    expect_eq "E2: never retried" "$(hook_calls "${dir}/hook.log" pre-commit)" "1"

    local spec label body
    local -a specs=(
        "delete-config|rm .pre-commit-config.yaml"
        "pre-commit-hooks-yaml|printf 'x\n' > .pre-commit-hooks.yaml"
        "husky|mkdir -p .husky && printf 'exit 0\n' > .husky/pre-commit"
        "lefthook|printf 'pre-commit: {}\n' > lefthook-local.yml"
        "secrets-baseline|printf '{}\n' > .secrets.baseline"
        "gitleaksignore|printf 'abc\n' > .gitleaksignore"
        "gitleaks-toml|printf '[allowlist]\n' > .gitleaks.toml"
        "trufflehogignore|printf 'x\n' > .trufflehogignore"
        "talismanrc|printf 'x\n' > .talismanrc"
        "lintstagedrc|printf '{}\n' > .lintstagedrc.json"
        "lint-staged-config|printf 'module.exports = {}\n' > lint-staged.config.js"
        "hook-file|printf '#!/bin/sh\nexit 0\n' > \"\$(git rev-parse --git-path hooks)/pre-commit\""
        "hook-symlink|ln -sf /bin/true \"\$(git rev-parse --git-path hooks)/commit-msg\""
        "hooks-path|git config core.hooksPath /nonexistent-hooks"
        "local-config|git config --local hookrepair.test 1"
    )
    for spec in "${specs[@]}"; do
        label="${spec%%|*}"; body="${spec#*|}"
        dir="${WORK}/e3-${label}"
        commit_case "${dir}"
        if [[ "${label}" == "delete-config" ]]; then
            # The config must be tracked before the round so deleting it is a change.
            git -C "${dir}/repo" reset -q -- src.txt
            printf 'repos: []\n' > "${dir}/repo/.pre-commit-config.yaml"
            git -C "${dir}/repo" add .pre-commit-config.yaml
            git -C "${dir}/repo" commit -qm "track pre-commit config"
            git -C "${dir}/repo" add src.txt
        fi
        STUB_AGENT_ACTION=fix STUB_AGENT_BODY="${body}" STUB_VERDICTS=CONTINUE run_driver "${dir}" "${COMMIT_ARGS[@]}"
        expect_eq "E3 ${label}: refused" "$(var_of "${dir}" STATUS)" "refused"
        [[ "$(var_of "${dir}" REASON)" == "hook-config-modified: "* ]] && pass \
            || fail "E3 ${label}: reason must start hook-config-modified (got '$(var_of "${dir}" REASON)')"
        expect_eq "E3 ${label}: never retried" "$(hook_calls "${dir}/hook.log" pre-commit)" "1"
        expect_eq "E3 ${label}: evaluator not called" "$(stub_calls "${dir}/stub" loop-health-evaluator)" "0"
    done

    # E4 changes the task made before the step are not refused.
    dir="${WORK}/e4-task-config"
    commit_case "${dir}"
    printf 'repos: []\n' > "${dir}/repo/.pre-commit-config.yaml"
    printf 'abc\n' > "${dir}/repo/.gitleaksignore"
    git -C "${dir}/repo" add .pre-commit-config.yaml .gitleaksignore
    STUB_AGENT_ACTION=fix run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "E4: the task's own config change is not refused" "$(var_of "${dir}" STATUS)" "ok"
    expect_eq "E4: config committed with the fix" "$(git -C "${dir}/repo" show HEAD:.pre-commit-config.yaml)" "repos: []"

    # E5 an inline secret-scanner suppression added by the agent is refused.
    local -a markers=(
        "pragma|printf 'key = 1  # pragma: allowlist secret\\n' > marker.py"
        "gitleaks|printf 'key = 1  # gitleaks:allow\\n' > marker.py"
        "trufflehog|printf 'key = 1  # trufflehog:ignore\\n' > marker.py"
    )
    for spec in "${markers[@]}"; do
        label="${spec%%|*}"; body="${spec#*|}"
        dir="${WORK}/e5-${label}"
        commit_case "${dir}"
        STUB_AGENT_ACTION=fix STUB_AGENT_BODY="${body}" STUB_VERDICTS=CONTINUE run_driver "${dir}" "${COMMIT_ARGS[@]}"
        expect_eq "E5 ${label}: refused" "$(var_of "${dir}" STATUS)" "refused"
        expect_eq "E5 ${label}: reason" "$(var_of "${dir}" REASON)" "scanner-suppression-added: marker.py"
        expect_eq "E5 ${label}: never retried" "$(hook_calls "${dir}/hook.log" pre-commit)" "1"
    done

    # E6 an edit to the script a symlinked hook points at is refused.
    dir="${WORK}/e6-linked-hook-target"
    commit_case "${dir}"
    local hd
    hd="$(hooks_dir "${dir}/repo")"
    mv "${hd}/pre-commit" "${dir}/linked-pre-commit.sh"
    ln -s "${dir}/linked-pre-commit.sh" "${hd}/pre-commit"
    STUB_AGENT_ACTION=fix STUB_AGENT_BODY='printf "# agent edit\n" >> "$(readlink "$(git rev-parse --git-path hooks)/pre-commit")"' \
        STUB_VERDICTS=CONTINUE run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "E6: refused" "$(var_of "${dir}" STATUS)" "refused"
    [[ "$(var_of "${dir}" REASON)" == "hook-config-modified: hooks/pre-commit"* ]] && pass \
        || fail "E6: reason must name hooks/pre-commit (got '$(var_of "${dir}" REASON)')"

    # E7 refs and the stash: a push, a stash or a moved remote-tracking ref
    # made during the round is refused even though HEAD stays where it was.
    dir="${WORK}/e7-stash"
    commit_case "${dir}"
    STUB_AGENT_ACTION=noop STUB_AGENT_BODY='git stash -q' STUB_VERDICTS=CONTINUE run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "E7 stash: refused" "$(var_of "${dir}" STATUS)" "refused"
    expect_eq "E7 stash: reason" "$(var_of "${dir}" REASON)" "agent-changed-refs: git-stash"
    expect_eq "E7 stash: never retried" "$(hook_calls "${dir}/hook.log" pre-commit)" "1"
    # A stash made in another worktree, on another HEAD, during the round is
    # another session's and does not refuse it.
    dir="${WORK}/e7-stash-other-worktree"
    commit_case "${dir}"
    printf 'm\n' > "${dir}/main/moved.txt"
    git -C "${dir}/main" add moved.txt
    git -C "${dir}/main" commit -q -m "main moved on"
    STUB_AGENT_ACTION=fix STUB_AGENT_BODY='printf "other\n" >> ../main/README.md && git -C ../main stash -q' \
        run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "E7 stash in another worktree: harness: a stash was made" "$(git -C "${dir}/main" stash list | wc -l | tr -d ' ')" "1"
    expect_eq "E7 stash in another worktree: not refused" "$(var_of "${dir}" STATUS)" "ok"
    dir="${WORK}/e7-update-ref"
    commit_case "${dir}"
    STUB_AGENT_ACTION=fix STUB_VERDICTS=CONTINUE \
        STUB_AGENT_BODY='git update-ref "refs/remotes/origin/$(git branch --show-current)" "$(git commit-tree "HEAD^{tree}" -p HEAD -m x)"' \
        run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "E7 update-ref: refused" "$(var_of "${dir}" STATUS)" "refused"
    expect_eq "E7 update-ref: reason" "$(var_of "${dir}" REASON)" "agent-changed-refs: git-refs"
    # Another branch, as a parallel session in another worktree would move it, is not refused.
    dir="${WORK}/e7-other-branch"
    commit_case "${dir}"
    # A real file, not --allow-empty: the harness hook rejects an empty commit.
    STUB_AGENT_ACTION=fix STUB_AGENT_BODY='printf "p\n" > ../main/parallel.txt && git -C ../main add parallel.txt && git -C ../main commit -q -m "parallel session"' \
        run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "E7 other branch: harness: the other branch moved" "$(git -C "${dir}/main" log -1 --format=%s)" "parallel session"
    expect_eq "E7 other branch: not refused" "$(var_of "${dir}" STATUS)" "ok"

    # E8 the script a `repo: local` hook runs is protected like the config.
    local entry_spec entry_label entry_line entry_path
    for entry_spec in "plain|entry: scripts/check.sh|scripts/check.sh" \
        "quoted|entry: \"./scripts/check.sh --strict\"|scripts/check.sh" \
        "list-item|- entry: scripts/check.sh|scripts/check.sh"; do
        IFS='|' read -r entry_label entry_line entry_path <<<"${entry_spec}"
        dir="${WORK}/e8-local-entry-${entry_label}"
        commit_case "${dir}"
        git -C "${dir}/repo" reset -q -- src.txt
        mkdir -p "${dir}/repo/scripts"
        printf '#!/bin/sh\nexit 0\n' > "${dir}/repo/scripts/check.sh"
        if [[ "${entry_label}" == "list-item" ]]; then
            # The entry as the first key of its list item.
            printf 'repos:\n  - repo: local\n    hooks:\n      %s\n        id: check\n        language: script\n' \
                "${entry_line}" > "${dir}/repo/.pre-commit-config.yaml"
        else
            printf 'repos:\n  - repo: https://github.com/example/hooks\n    rev: v1\n    hooks:\n      - id: remote\n        entry: scripts/remote-only.sh\n  - repo: local\n    hooks:\n      - id: check\n        name: check\n        language: script\n        %s\n' \
                "${entry_line}" > "${dir}/repo/.pre-commit-config.yaml"
        fi
        git -C "${dir}/repo" add .pre-commit-config.yaml scripts/check.sh
        git -C "${dir}/repo" commit -qm "local hook"
        git -C "${dir}/repo" add src.txt
        STUB_AGENT_ACTION=fix STUB_AGENT_BODY="printf 'exit 0 # weakened\n' >> ${entry_path}" STUB_VERDICTS=CONTINUE \
            run_driver "${dir}" "${COMMIT_ARGS[@]}"
        expect_eq "E8 ${entry_label}: refused" "$(var_of "${dir}" STATUS)" "refused"
        expect_eq "E8 ${entry_label}: reason names the entry script" "$(var_of "${dir}" REASON)" "hook-config-modified: ${entry_path}"
    done
    # A remote repo's entry is relative to that repo, not this one: not protected.
    dir="${WORK}/e8-remote-entry"
    commit_case "${dir}"
    git -C "${dir}/repo" reset -q -- src.txt
    mkdir -p "${dir}/repo/scripts"
    printf 'x\n' > "${dir}/repo/scripts/remote-only.sh"
    printf 'repos:\n  - repo: https://github.com/example/hooks\n    rev: v1\n    hooks:\n      - id: remote\n        entry: scripts/remote-only.sh\n' \
        > "${dir}/repo/.pre-commit-config.yaml"
    git -C "${dir}/repo" add .pre-commit-config.yaml scripts/remote-only.sh
    git -C "${dir}/repo" commit -qm "remote hook"
    git -C "${dir}/repo" add src.txt
    STUB_AGENT_ACTION=fix STUB_AGENT_BODY="printf 'y\n' >> scripts/remote-only.sh" run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "E8 remote entry: not refused" "$(var_of "${dir}" STATUS)" "ok"

    # E9 lint configuration, at the top and nested, and Cargo.toml lint tables.
    local -a lint_specs=(
        "clippy-toml|clippy.toml|printf 'too-many-arguments-threshold = 99\n' > clippy.toml"
        "dot-clippy-nested|crates/x/.clippy.toml|mkdir -p crates/x && printf 'x = 1\n' > crates/x/.clippy.toml"
        "rustfmt-toml|rustfmt.toml|printf 'disable_all_formatting = true\n' > rustfmt.toml"
        "dot-rustfmt|.rustfmt.toml|printf 'x = 1\n' > .rustfmt.toml"
        "cargo-config|.cargo/config.toml|mkdir -p .cargo && printf '[build]\nrustflags = [\"-Awarnings\"]\n' > .cargo/config.toml"
        "cargo-config-legacy|.cargo/config|mkdir -p .cargo && printf '[build]\n' > .cargo/config"
        "cargo-lints|Cargo.toml[lints]|sed -i 's/all = \"deny\"/all = \"allow\"/' Cargo.toml"
        "cargo-workspace-lints|crates/y/Cargo.toml[lints]|printf '[workspace.lints.rust]\nwarnings = \"allow\"\n' >> crates/y/Cargo.toml"
    )
    for spec in "${lint_specs[@]}"; do
        IFS='|' read -r label entry_path body <<<"${spec}"
        dir="${WORK}/e9-${label}"
        commit_case "${dir}"
        git -C "${dir}/repo" reset -q -- src.txt
        mkdir -p "${dir}/repo/crates/y"
        printf '[package]\nname = "x"\n\n[dependencies]\n\n[lints.clippy]\nall = "deny"\n' > "${dir}/repo/Cargo.toml"
        printf '[package]\nname = "y"\n' > "${dir}/repo/crates/y/Cargo.toml"
        git -C "${dir}/repo" add Cargo.toml crates/y/Cargo.toml
        git -C "${dir}/repo" commit -qm "cargo manifests"
        git -C "${dir}/repo" add src.txt
        STUB_AGENT_ACTION=fix STUB_AGENT_BODY="${body}" STUB_VERDICTS=CONTINUE run_driver "${dir}" "${COMMIT_ARGS[@]}"
        expect_eq "E9 ${label}: refused" "$(var_of "${dir}" STATUS)" "refused"
        expect_eq "E9 ${label}: reason" "$(var_of "${dir}" REASON)" "hook-config-modified: ${entry_path}"
    done
    # A Cargo.toml change outside the lint tables is not refused.
    dir="${WORK}/e9-cargo-dependency"
    commit_case "${dir}"
    git -C "${dir}/repo" reset -q -- src.txt
    printf '[package]\nname = "x"\n\n[dependencies]\n\n[lints.clippy]\nall = "deny"\n' > "${dir}/repo/Cargo.toml"
    git -C "${dir}/repo" add Cargo.toml
    git -C "${dir}/repo" commit -qm "cargo manifest"
    git -C "${dir}/repo" add src.txt
    STUB_AGENT_ACTION=fix STUB_AGENT_BODY="sed -i 's/^\[dependencies\]\$/[dependencies]\nserde = \"1\"/' Cargo.toml" \
        run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "E9 dependency: not refused" "$(var_of "${dir}" STATUS)" "ok"
    expect_eq "E9 dependency: the edit was committed" "$(git -C "${dir}/repo" show HEAD:Cargo.toml | grep -c '^serde = "1"$' || true)" "1"

    # E10 an inline linter suppression or skipped test added by the agent is refused.
    local -a lint_markers=(
        "allow|lib.rs|printf '#[allow(dead_code)]\nfn x() {}\n' > lib.rs"
        "inner-allow|lib.rs|printf '#![allow(clippy::all)]\n' > lib.rs"
        "expect|lib.rs|printf '#[expect(unused)]\nfn x() {}\n' > lib.rs"
        "ignore|t.rs|printf '#[test]\n#[ignore]\nfn t() {}\n' > t.rs"
        "noqa|m.py|printf 'import os  # noqa: F401\n' > m.py"
        "eslint|a.js|printf '/* eslint-disable */\n' > a.js"
        "ts-ignore|a.ts|printf '// @ts-ignore\nlet x = 1\n' > a.ts"
        "shellcheck|a.sh|printf '# shellcheck disable=SC2086\necho \$x\n' > a.sh"
    )
    for spec in "${lint_markers[@]}"; do
        IFS='|' read -r label entry_path body <<<"${spec}"
        dir="${WORK}/e10-${label}"
        commit_case "${dir}"
        STUB_AGENT_ACTION=fix STUB_AGENT_BODY="${body}" STUB_VERDICTS=CONTINUE run_driver "${dir}" "${COMMIT_ARGS[@]}"
        expect_eq "E10 ${label}: refused" "$(var_of "${dir}" STATUS)" "refused"
        expect_eq "E10 ${label}: reason" "$(var_of "${dir}" REASON)" "lint-suppression-added: ${entry_path}"
        expect_eq "E10 ${label}: never retried" "$(hook_calls "${dir}/hook.log" pre-commit)" "1"
    done
    # A line that already carried a suppression and is edited by the fix is not
    # a new suppression.
    dir="${WORK}/e10-existing"
    commit_case "${dir}" "FORMAT_ME #[allow(dead_code)] # pragma: allowlist secret"
    STUB_AGENT_ACTION=fix run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "E10 existing suppression on an edited line: not refused" "$(var_of "${dir}" STATUS)" "ok"

    # E11 `git rm --cached` of a tracked file the worktree keeps is refused.
    dir="${WORK}/e11-rm-cached"
    commit_case "${dir}"
    git -C "${dir}/repo" reset -q -- src.txt
    printf 'tracked\n' > "${dir}/repo/keep.txt"
    git -C "${dir}/repo" add keep.txt
    git -C "${dir}/repo" commit -qm "keep"
    git -C "${dir}/repo" add src.txt
    STUB_AGENT_ACTION=fix STUB_AGENT_BODY='git rm -q --cached keep.txt && echo keep.txt >> .gitignore' STUB_VERDICTS=CONTINUE \
        run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "E11: refused" "$(var_of "${dir}" STATUS)" "refused"
    expect_eq "E11: reason" "$(var_of "${dir}" REASON)" "tracked-file-untracked: keep.txt"
    expect_eq "E11: never retried" "$(hook_calls "${dir}/hook.log" pre-commit)" "1"
}

# =============================================================================
# F. redaction and the result line
# =============================================================================
section_f() {
    need "${DRIVER}" F || return 0
    local dir="${WORK}/f1-redact" secret
    commit_case "${dir}"
    # Built by concatenation so no secret scanner flags this file.
    local gh="gh""p_""AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    local pat="github""_pat_""11ABCDEFG0123456789_abcdefghijklmnop"
    local ant="sk-""ant-""api03-ANTHROPICKEYVALUE"
    local sk="sk-""OPENAIKEYVALUE0123456789abcdef"
    local xox="xo""xb-""123456-SLACKTOKENVALUE"
    local akia="AK""IA""IOSFODNN7EXAMPLE"
    # Split like the others so secret scanners do not flag the fixtures.
    local glpat="gl""pat-abcdefghij0123456789" gkey="AI""za" npmtok="np""m_" jwt="ey""J"
    gkey="${gkey}SyFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE1"
    npmtok="${npmtok}FAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE"
    jwt="${jwt}hbGciOiJIUzI1NiJ9.eyJzdWIiOiJmYWtlIn0.fakesignature"
    local -a secrets=(
        "s3cr3tpw" "${gh}" "${pat}" "zzAuthHeaderValuezz" "bearerpayloadvalue"
        "${ant}" "${sk}" "${xox}" "${akia}" "MIIEPEMBODYLINE"
        "gitleaksSecretValue" "gitleaksMatchValue" "hunter2pw" "apikeyvalue9"
        "tokval77" "clisecret5" "pwdval3"
        "hunter2colon" "abc123colon" "zzzdashkey" "yyyplainkey"
        "jsonpwvalue" "${glpat}" "${gkey}" "${npmtok}" "${jwt}" "azacctkeyvalue"
    )
    # Keys and values are kept apart so the fixture holds no credential-like
    # literal for a secret scanner to flag.
    local k_pw="pass""word" v_url="s3cr3t""pw" v_match="gitleaks""MatchValue" v_pw="hunter2""pw"
    local v_api="apikey""value9" v_tok="tok""val77" v_cli="cli""secret5" v_pwd="pwd""val3"
    local v_colon="hunter2""colon" v_json="json""pw" v_tokc="abc123""colon" v_dash="zzzdash""key"
    local v_plain="yyyplain""key" v_az="azacct""keyvalue" v_auth="zzAuthHeader""Valuezz"
    {
        echo "remote: https://alice:${v_url}@example.com/repo.git"
        echo "token ${gh} and ${pat}"
        echo "Authorization: token ${v_auth}"
        echo "header Bearer bearerpayloadvalue"
        echo "keys ${ant} ${sk} ${xox} ${akia}"
        echo "-----BEGIN RSA ""PRIVATE KEY-----"
        echo "MIIEPEMBODYLINE"
        echo "-----END RSA ""PRIVATE KEY-----"
        echo "Secret:      gitleaksSecretValue"
        echo "Match:       ${k_pw} = ${v_match}"
        echo "${k_pw}=${v_pw} api_key=${v_api} MY_TOKEN=${v_tok} client_secret=${v_cli} passwd=${v_pwd}"
        echo "${k_pw}: ${v_colon}"
        echo "token: ${v_tokc}"
        echo "api-key=${v_dash} apikey=${v_plain}"
        echo "{\"${k_pw}\": \"${v_json}\" , \"db_${k_pw}\": \"${v_json}value\"}"
        echo "tokens ${glpat} ${gkey} ${npmtok} ${jwt}"
        echo "AccountName=acct;AccountKey=${v_az};EndpointSuffix=x"
        echo "user""name=alice_visible"
    } > "${dir}/extra"
    HOOK_EXTRA_OUTPUT_FILE="${dir}/extra" STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "F1: status stuck" "$(var_of "${dir}" STATUS)" "stuck"
    local hr
    hr="$(var_of "${dir}" EVIDENCE_DIR)"
    [[ -f "${dir}/stub/evidence.1" ]] || fail "F1: agent saw no evidence file"
    : > "${dir}/eval.argv"; : > "${dir}/agent.argv"
    [[ ! -f "${dir}/stub/loop-health-evaluator.1.argv" ]] || tr '\0' '\n' < "${dir}/stub/loop-health-evaluator.1.argv" > "${dir}/eval.argv"
    [[ ! -f "${dir}/stub/hook-rejection-repair.1.argv" ]] || tr '\0' '\n' < "${dir}/stub/hook-rejection-repair.1.argv" > "${dir}/agent.argv"
    for secret in "${secrets[@]}"; do
        expect_file_lacks "F1: '${secret:0:12}' redacted in the agent's evidence" "${dir}/stub/evidence.1" "${secret}"
        expect_file_lacks "F1: '${secret:0:12}' redacted on stderr" "${dir}/stderr" "${secret}"
        expect_file_lacks "F1: '${secret:0:12}' redacted in evaluator inputs" "${dir}/eval.argv" "${secret}"
        expect_file_lacks "F1: '${secret:0:12}' never in agent argv" "${dir}/agent.argv" "${secret}"
        expect_file_lacks "F1: '${secret:0:12}' redacted in kept evidence" "${hr}/round-1.evidence" "${secret}"
    done
    expect_file_contains "F1: key=value redaction covers only the listed keys" "${dir}/stub/evidence.1" "username=alice_visible"
    expect_file_contains "F1: unredacted hook text is kept" "${dir}/stub/evidence.1" "example.com/repo.git"

    # F2 hostile evaluator reasoning still yields a valid JSON line.
    dir="${WORK}/f2-json"
    commit_case "${dir}"
    STUB_HEALTH_REASON="$(printf 'quote " backslash \\ tab \t bell \001 end')" STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK \
        run_driver "${dir}" "${COMMIT_ARGS[@]}"
    local line
    line="$(result_line "${dir}")"
    if [[ -n "${line}" ]] && printf '%s' "${line}" | python3 -I -c 'import json,sys; json.loads(sys.stdin.read())' 2>/dev/null; then
        pass
    else
        fail "F2: the result line must be valid JSON with hostile reasoning (got '${line}')"
    fi
    expect_eq "F2: exactly one result line" "$(grep -cE '^\{"hook_repair_result":' "${dir}/stderr" || true)" "1"
    expect_eq "F2: result rounds is a number" "$(result_field "${dir}" 'type(r["rounds"]).__name__')" "int"
    expect_eq "F2: version is a number" "$(result_field "${dir}" 'type(r["version"]).__name__')" "int"
    expect_eq "F2: hook_ids is a list" "$(result_field "${dir}" 'type(r["hook_ids"]).__name__')" "list"
    [[ "$(result_field "${dir}" 'r["reasoning"]')" != *'"'* ]] && pass || fail "F2: quotes stripped from reasoning"

    dir="${WORK}/f3-unknown"
    commit_case "${dir}"
    HOOK_OWN_ID="" STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "F3: hook_ids is [\"unknown\"] when none parsed" "$(result_field "${dir}" 'r["hook_ids"]')" "unknown"
}

# =============================================================================
# G. push classification and push repair
# =============================================================================
# push_case <case-dir>: a repo one commit ahead of its upstream with the
# pre-push hook. The pushed tree contains PUSH_BAD unless $2 says otherwise.
push_case() {
    local dir="$1" content="${2:-PUSH_BAD}"
    new_repo "${dir}"
    install_pre_commit_hook "${dir}/repo"
    install_pre_push_hook "${dir}/repo"
    printf '%s\n' "${content}" > "${dir}/repo/src.txt"
    git -C "${dir}/repo" add src.txt
    ( cd "${dir}/repo" && git commit -qm "feat: the change" ) >/dev/null
}
PUSH_ARGS=(push step-15-commit-push --fix-message "fix: address {hook_ids} pre-push rejection (#1671)" -- git push)

section_g() {
    need "${DRIVER}" G || return 0
    local dir feat

    # G1 rejected, fixed: a new fix commit, original not amended, push lands.
    dir="${WORK}/g1-push-fixed"
    push_case "${dir}"
    feat="$(git -C "${dir}/repo" rev-parse HEAD)"
    STUB_AGENT_ACTION=fix run_driver "${dir}" "${PUSH_ARGS[@]}"
    expect_eq "G1: rc 0" "$(rc_of "${dir}")" "0"
    expect_eq "G1: status ok" "$(var_of "${dir}" STATUS)" "ok"
    expect_eq "G1: ids" "$(var_of "${dir}" IDS)" "cargo-test"
    expect_eq "G1: fix commit message" "$(git -C "${dir}/repo" log -1 --format=%s)" "fix: address cargo-test pre-push rejection (#1671)"
    expect_eq "G1: original commit not amended" "$(git -C "${dir}/repo" rev-parse HEAD~1 2>/dev/null)" "${feat}"
    expect_eq "G1: remote has the fix" "$(git -C "${dir}/repo" rev-parse '@{u}')" "$(git -C "${dir}/repo" rev-parse HEAD)"
    expect_eq "G1: hook_repair_operation push" "$(stub_arg "${dir}/stub" hook-rejection-repair 1 hook_repair_operation)" "push"
    expect_eq "G1: evaluator not called" "$(stub_calls "${dir}/stub" loop-health-evaluator)" "0"
    expect_driver_hygiene G1 "${dir}"

    # G2 the agent changed nothing: the push is retried as is, no fix commit.
    dir="${WORK}/g2-push-unchanged"
    push_case "${dir}"
    feat="$(git -C "${dir}/repo" rev-parse HEAD)"
    STUB_AGENT_ACTION=noop STUB_VERDICTS="CONTINUE STUCK" run_driver "${dir}" "${PUSH_ARGS[@]}"
    expect_eq "G2: status stuck" "$(var_of "${dir}" STATUS)" "stuck"
    expect_eq "G2: rounds 2" "$(var_of "${dir}" ROUNDS)" "2"
    expect_eq "G2: HEAD unchanged (no empty fix commit)" "$(git -C "${dir}/repo" rev-parse HEAD)" "${feat}"
    expect_eq "G2: push attempted three times" "$(hook_calls "${dir}/hook.log" pre-push)" "3"
    expect_eq "G2: result operation push" "$(result_field "${dir}" 'r["operation"]')" "push"
    expect_eq "G2: loop_name" "$(stub_arg "${dir}/stub" loop-health-evaluator 1 loop_name)" "hook-repair:step-15-commit-push:push"

    # G3 the fix commit is itself rejected: repaired one level deep; the inner
    # loop's failure fails the step and the push is not retried.
    dir="${WORK}/g3-nested-stuck"
    push_case "${dir}"
    STUB_FIX_REPLACEMENT=FORMAT_ME STUB_AGENT_ACTION="fix noop" STUB_VERDICTS=STUCK run_driver "${dir}" "${PUSH_ARGS[@]}"
    expect_eq "G3: rc 1" "$(rc_of "${dir}")" "1"
    expect_eq "G3: inner status stuck" "$(var_of "${dir}" STATUS)" "stuck"
    expect_eq "G3: push not retried after the inner failure" "$(hook_calls "${dir}/hook.log" pre-push)" "1"
    expect_eq "G3: inner round repairs the commit" "$(stub_arg "${dir}/stub" hook-rejection-repair 2 hook_repair_operation)" "commit"
    expect_eq "G3: two agent rounds" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "2"
    expect_eq "G3: no fix commit landed" "$(git -C "${dir}/repo" log -1 --format=%s)" "feat: the change"

    # G4 the fix commit is rejected, then repaired: push lands.
    dir="${WORK}/g4-nested-fixed"
    push_case "${dir}"
    STUB_FIX_REPLACEMENT=FORMAT_ME STUB_AGENT_ACTION="fix fix" run_driver "${dir}" "${PUSH_ARGS[@]}"
    expect_eq "G4: status ok" "$(var_of "${dir}" STATUS)" "ok"
    expect_eq "G4: fix commit message" "$(git -C "${dir}/repo" log -1 --format=%s)" "fix: address cargo-test pre-push rejection (#1671)"
    expect_eq "G4: pushed" "$(git -C "${dir}/repo" rev-parse '@{u}')" "$(git -C "${dir}/repo" rev-parse HEAD)"

    # G5 hook output with network-looking words is still a hook rejection:
    # only git's own lines are searched for the markers.
    dir="${WORK}/g5-hook-says-network"
    push_case "${dir}"
    printf 'requests.exceptions.ConnectionError: Connection refused\ntest timed out\nPermission denied (test fixture)\n' > "${dir}/extra"
    HOOK_EXTRA_OUTPUT_FILE="${dir}/extra" STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK run_driver "${dir}" "${PUSH_ARGS[@]}"
    expect_eq "G5: hook text is not searched for network markers" "$(var_of "${dir}" STATUS)" "stuck"

    # G6 non-fast-forward with a passing pre-push hook: not_hook.
    dir="${WORK}/g6-non-ff"
    push_case "${dir}" "clean"
    git clone -q "${dir}/origin.git" "${dir}/other" 2>/dev/null
    configure_identity "${dir}/other"
    git -C "${dir}/other" checkout -q "feat/issue-1671-g6-non-ff"
    printf 'other\n' > "${dir}/other/other.txt"
    git -C "${dir}/other" add other.txt && git -C "${dir}/other" commit -qm other && git -C "${dir}/other" push -q 2>/dev/null
    run_driver "${dir}" "${PUSH_ARGS[@]}"
    expect_eq "G6: non-fast-forward is not_hook" "$(var_of "${dir}" STATUS)" "not_hook"
    expect_ne "G6: rc is the push's non-zero rc" "$(rc_of "${dir}")" "0"
    expect_eq "G6: no agent ran" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"

    # G7 no pre-push hook: not_hook.
    dir="${WORK}/g7-no-hook"
    push_case "${dir}"
    rm -f "$(hooks_dir "${dir}/repo")/pre-push"
    git -C "${dir}/repo" remote set-url --push origin "${dir}/missing.git"
    run_driver "${dir}" "${PUSH_ARGS[@]}"
    expect_eq "G7: no pre-push hook is not_hook" "$(var_of "${dir}" STATUS)" "not_hook"

    # G8 unreachable remote with a hook present: git's own fatal: line excludes it.
    dir="${WORK}/g8-unreachable"
    push_case "${dir}"
    git -C "${dir}/repo" remote set-url --push origin "${dir}/missing.git"
    run_driver "${dir}" "${PUSH_ARGS[@]}"
    expect_eq "G8: unreachable remote is not_hook" "$(var_of "${dir}" STATUS)" "not_hook"
    expect_eq "G8: no agent ran" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"

    # G9 the documented gap: a hook's own `error: ... timed out` line is not_hook.
    dir="${WORK}/g9-gap"
    push_case "${dir}"
    printf 'error: request timed out\n' > "${dir}/extra"
    HOOK_EXTRA_OUTPUT_FILE="${dir}/extra" run_driver "${dir}" "${PUSH_ARGS[@]}"
    expect_eq "G9: hook line starting error: with a marker is not_hook (documented gap)" "$(var_of "${dir}" STATUS)" "not_hook"

    # G10 refusal on push.
    dir="${WORK}/g10-push-refused"
    push_case "${dir}"
    STUB_AGENT_ACTION=edit-pre-commit-config run_driver "${dir}" "${PUSH_ARGS[@]}"
    expect_eq "G10: refused" "$(var_of "${dir}" STATUS)" "refused"
    expect_eq "G10: push not retried" "$(hook_calls "${dir}/hook.log" pre-push)" "1"

    # G11 the agent pushes past the rejecting pre-push hook itself: the moved
    # remote-tracking ref refuses the round, and the operation is not ok.
    dir="${WORK}/g11-agent-pushes"
    push_case "${dir}"
    feat="$(git -C "${dir}/repo" rev-parse HEAD)"
    STUB_AGENT_ACTION=noop STUB_AGENT_BODY='git push -q --no-verify 2>/dev/null' STUB_VERDICTS=CONTINUE \
        run_driver "${dir}" "${PUSH_ARGS[@]}"
    expect_eq "G11: harness: the agent's push reached the remote" "$(git -C "${dir}/origin.git" rev-parse "feat/issue-1671-g11-agent-pushes")" "${feat}"
    expect_eq "G11: rc 1" "$(rc_of "${dir}")" "1"
    expect_eq "G11: refused" "$(var_of "${dir}" STATUS)" "refused"
    expect_eq "G11: reason" "$(var_of "${dir}" REASON)" "agent-changed-refs: git-refs"
    expect_eq "G11: push not retried" "$(hook_calls "${dir}/hook.log" pre-push)" "1"
    expect_eq "G11: evaluator not called" "$(stub_calls "${dir}/stub" loop-health-evaluator)" "0"

    # G12 a git dir that cannot hold the attempt file: the command still runs,
    # its output is redacted on stderr and in --output, and a failure is not
    # repaired.
    local gd tok="gh""p_""CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC" g12
    for g12 in rejected clean; do
        dir="${WORK}/g12-gitdir-readonly-${g12}"
        if [[ "${g12}" == rejected ]]; then push_case "${dir}"; else push_case "${dir}" "clean"; fi
        printf 'token %s\n' "${tok}" > "${dir}/extra"
        gd="$(git -C "${dir}/repo" rev-parse --absolute-git-dir)"
        chmod 0500 "${gd}"
        HOOK_EXTRA_OUTPUT_FILE="${dir}/extra" run_driver "${dir}" push step-15-commit-push \
            --fix-message "fix: address {hook_ids} pre-push rejection" --output "${dir}/push.out" -- git push
        chmod 0700 "${gd}"
        expect_eq "G12 ${g12}: no agent ran" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"
        expect_file_lacks "G12 ${g12}: no raw token on stderr" "${dir}/stderr" "${tok}"
        expect_file_lacks "G12 ${g12}: no raw token in --output" "${dir}/push.out" "${tok}"
        expect_empty_file "G12 ${g12}: nothing on stdout" "${dir}/stdout"
    done
    dir="${WORK}/g12-gitdir-readonly-rejected"
    expect_eq "G12 rejected: not_hook" "$(var_of "${dir}" STATUS)" "not_hook"
    expect_ne "G12 rejected: rc non-zero" "$(rc_of "${dir}")" "0"
    expect_file_contains "G12 rejected: --output holds the hook output" "${dir}/push.out" "cargo test"
    expect_file_contains "G12 rejected: --output is redacted" "${dir}/push.out" "REDACTED"
    expect_file_contains "G12 rejected: stderr holds the hook output" "${dir}/stderr" "cargo test"
    dir="${WORK}/g12-gitdir-readonly-clean"
    expect_eq "G12 clean: ok" "$(var_of "${dir}" STATUS)" "ok"
    expect_eq "G12 clean: rc 0" "$(rc_of "${dir}")" "0"
    expect_eq "G12 clean: pushed" "$(git -C "${dir}/repo" rev-parse '@{u}')" "$(git -C "${dir}/repo" rev-parse HEAD)"
}

# =============================================================================
# H. the step bodies
# =============================================================================
extract_step_command() {
    local recipe="$1" step_id="$2" output="$3"
    awk -v step_id="${step_id}" '
        $0 ~ "^[[:space:]]*- id: \"" step_id "\"[[:space:]]*$" { in_step = 1; next }
        in_step && /^[[:space:]]*- id: / { exit }
        in_step && /^[[:space:]]*command: \|[[:space:]]*$/ { in_command = 1; next }
        in_command {
            if (substr($0, 1, 6) == "      ") { print substr($0, 7); next }
            if ($0 ~ /^[[:space:]]*$/) { print ""; next }
            exit
        }
    ' "${recipe}" > "${output}"
    if [[ ! -s "${output}" ]]; then
        echo "HARNESS-ERROR: could not extract ${step_id} from ${recipe}" >&2
        exit 2
    fi
}

STEP_TDD="${WORK}/checkpoint-after-implementation.sh"
STEP_REFACTOR="${WORK}/checkpoint-after-review-feedback.sh"
STEP_15="${WORK}/step-15-commit-push.sh"
extract_step_command "${TDD_RECIPE}" "checkpoint-after-implementation" "${STEP_TDD}"
extract_step_command "${REFACTOR_RECIPE}" "checkpoint-after-review-feedback" "${STEP_REFACTOR}"
extract_step_command "${PUBLISH_RECIPE}" "step-15-commit-push" "${STEP_15}"

section_h_static() {
    local label file id
    for spec in "checkpoint-after-implementation|${STEP_TDD}" "checkpoint-after-review-feedback|${STEP_REFACTOR}" "step-15-commit-push|${STEP_15}"; do
        id="${spec%%|*}"; file="${spec#*|}"; label="H-static ${id}"
        if grep -nE '(^|[;&|[:space:]])git[[:space:]]+add[[:space:]]+-A' "${file}" >/dev/null; then
            fail "${label}: git add -A must be replaced by stage_workflow_changes"
        else pass; fi
        # #1672 scopes the TDD checkpoint's initial add to the paths captured
        # before validation; the other two stage with stage_workflow_changes.
        local stage_text="stage_workflow_changes"
        [[ "${id}" == "checkpoint-after-implementation" ]] && stage_text="git --literal-pathspecs add -A --pathspec-from-file"
        expect_file_contains "${label}: initial staging (${stage_text})" "${file}" "${stage_text}"
        expect_file_contains "${label}: commit goes through hook_repair_run" "${file}" "hook_repair_run commit ${id}"
        if [[ "${id}" == "step-15-commit-push" ]]; then
            # Step-15's commit stays a plain `git commit`: PRE_COMMIT_ALLOW_NO_CONFIG
            # is not set for it, as before #1671.
            expect_eq "${label}: no guarded commit" "$(grep -c 'commit_with_pre_commit_guard' "${file}" || true)" "0"
            expect_file_contains "${label}: plain git commit inside hook_repair_run" "${file}" '-- git commit -m "$COMMIT_MSG"'
        else
            expect_eq "${label}: exactly one guarded commit" "$(grep -c 'commit_with_pre_commit_guard -m' "${file}")" "1"
        fi
        if grep -nE '^[[:space:]]*(PRE_COMMIT_ALLOW_NO_CONFIG=1[[:space:]]+)?git[[:space:]]+commit[[:space:]]+-m' "${file}" >/dev/null; then
            fail "${label}: a bare git commit -m remains"
        else pass; fi
        if grep -nE -- '--no-verify|(^|[[:space:]])SKIP=|core\.hooksPath' "${file}" >/dev/null; then
            fail "${label}: hook bypass text in the step body"
        else pass; fi
        expect_file_contains "${label}: WORKFLOW_HOOK_REPAIR_HELPER override" "${file}" "WORKFLOW_HOOK_REPAIR_HELPER"
        expect_file_contains "${label}: WORKFLOW_STAGE_CHANGES_HELPER override" "${file}" "WORKFLOW_STAGE_CHANGES_HELPER"
        # artifact-guard still runs before and after staging, and staging sits between.
        local first stage second
        first="$(grep -n 'artifact-guard --repo . --mode pre-publish' "${file}" | sed -n 1p | cut -d: -f1 || true)"
        second="$(grep -n 'artifact-guard --repo . --mode pre-publish' "${file}" | sed -n 2p | cut -d: -f1 || true)"
        stage="$(grep -nF -- "${stage_text}" "${file}" | grep -v '^[0-9]*:[[:space:]]*#' | grep -v 'WORKFLOW_STAGE_CHANGES_HELPER' | sed -n 1p | cut -d: -f1 || true)"
        if [[ -n "${first}" && -n "${stage}" && -n "${second}" ]] && (( first < stage && stage <= second )); then pass; else
            fail "${label}: ${stage_text} must run between the two artifact-guard calls (lines ${first:-?} ${stage:-?} ${second:-?})"
        fi
    done
    expect_file_contains "H-static step-15: push goes through hook_repair_run" "${STEP_15}" "hook_repair_run push step-15-commit-push"
    expect_file_contains "H-static step-15: fix message template" "${STEP_15}" 'fix: address {hook_ids} pre-push rejection'
    expect_file_contains "H-static step-15: orchestration-unavailable reason" "${STEP_15}" "orchestration-unavailable"
    expect_file_contains "H-static step-15: commit-failed reason" "${STEP_15}" "commit-failed"
    expect_file_contains "H-static step-15: #978 divergence check kept" "${STEP_15}" "diverged-upstream"
    expect_file_contains "H-static step-15: #1269 verification kept" "${STEP_15}" "verify_pushed_ref.sh"
}

# run_step <case-dir> <step-file> [VAR=value...]: run an extracted step body as
# the recipe runner would. STEP_PATH_PREFIX puts shims in front of the stub.
run_step() {
    local dir="$1" step="$2"
    shift 2
    mkdir -p "${dir}/home"
    if env "$@" \
        STUB_STATE="${dir}/stub" AMPLIHACK_BIN="${STUB_BIN}/amplihack" PATH="${STEP_PATH_PREFIX:-}${STUB_BIN}:${PATH}" \
        HOOK_LOG="${dir}/hook.log" TMPDIR="${WORK}/tmp" HOME="${dir}/home" \
        AMPLIHACK_HOME="${REPO_ROOT}" REPO_PATH="${dir}/main" \
        WORKFLOW_HOOK_REPAIR_HELPER="${RUN_STEP_HOOK_HELPER:-${DRIVER}}" WORKFLOW_STAGE_CHANGES_HELPER="${STAGE_HELPER}" \
        WORKTREE_SETUP_WORKTREE_PATH="${dir}/repo" \
        RECIPE_VAR_worktree_setup="$(jq -nc --arg p "${dir}/repo" '{worktree_path: $p}')" \
        bash "${step}" >"${dir}/step.out" 2>"${dir}/step.err"; then
        echo 0 > "${dir}/rc"
    else
        echo "$?" > "${dir}/rc"
    fi
}
step15_field() {
    tail -n 1 "$1/step.out" 2>/dev/null | jq -r ".$2" 2>/dev/null || echo "__invalid__"
}
# Step-15's stdout must be exactly the commit_result line; the commit's own
# output now goes through hook_repair_run, which writes nothing to stdout.
expect_one_stdout_line() {
    expect_eq "$1: step-15 stdout is exactly one line" "$(grep -c . "$2/step.out" 2>/dev/null || true)" "1"
}
STEP15_ENV=(TASK_DESCRIPTION="add the widget" ISSUE_NUMBER=1671 REMOTE_HOST_TYPE=github VERSION_BUMP_CHANGE_CLASSIFICATION=MINOR)

# checkpoint_case <case-dir>: uncommitted work containing FORMAT_ME plus
# artefacts the staging helper must leave out.
checkpoint_case() {
    local dir="$1"
    new_repo "${dir}"
    install_pre_commit_hook "${dir}/repo"
    printf 'FORMAT_ME\n' > "${dir}/repo/src.txt"
    mkdir -p "${dir}/repo/node_modules/pkg" "${dir}/repo/py/pkg.egg-info"
    printf 'x\n' > "${dir}/repo/node_modules/pkg/index.js"
    printf 'x\n' > "${dir}/repo/py/pkg.egg-info/PKG-INFO"
}

section_h_dynamic() {
    need "${DRIVER}" H || return 0
    need "${STAGE_HELPER}" H || return 0
    local dir seed

    # H1 TDD checkpoint: rejected, fixed, committed without artefacts.
    dir="${WORK}/h1-tdd-fixed"
    checkpoint_case "${dir}"
    seed="$(git -C "${dir}/repo" rev-parse HEAD)"
    # The agent's session writes amplihack runtime state into the worktree.
    run_step "${dir}" "${STEP_TDD}" STUB_AGENT_ACTION=fix \
        STUB_AGENT_BODY='mkdir -p .claude/runtime .amplihack && echo s > .claude/runtime/sessions.jsonl && echo g > .amplihack/graph_db'
    expect_eq "H1: exit 0" "$(rc_of "${dir}")" "0"
    expect_eq "H1: one checkpoint commit" "$(git -C "${dir}/repo" rev-parse HEAD~1 2>/dev/null)" "${seed}"
    expect_eq "H1: checkpoint message" "$(git -C "${dir}/repo" log -1 --format=%s)" "wip: checkpoint after implementation (steps 7-8)"
    expect_eq "H1: fixed content committed" "$(git -C "${dir}/repo" show HEAD:src.txt 2>/dev/null)" "formatted"
    expect_eq "H1: node_modules not committed" "$(git -C "${dir}/repo" ls-tree -r --name-only HEAD | grep -c node_modules || true)" "0"
    expect_eq "H1: egg-info not committed" "$(git -C "${dir}/repo" ls-tree -r --name-only HEAD | grep -c egg-info || true)" "0"
    expect_file_contains "H1: skipped artefacts reported" "${dir}/step.err" "stage_workflow_changes: skipped node_modules/pkg/index.js"
    expect_eq "H1: agent runtime state not committed" \
        "$(git -C "${dir}/repo" ls-tree -r --name-only HEAD | grep -cE '^(\.claude/runtime|\.amplihack)/' || true)" "0"
    expect_eq "H1: artifact-guard ran before staging, after it, and after the round" \
        "$(grep -c '^artifact-guard' "${dir}/stub/log" 2>/dev/null || true)" "3"
    expect_eq "H1: second artifact-guard saw the helper's staging" \
        "$(grep '^artifact-guard' "${dir}/stub/log" 2>/dev/null | sed -n 2p)" "artifact-guard staged=src.txt,"
    expect_eq "H1: third artifact-guard saw the agent's fix staged" \
        "$(grep '^artifact-guard' "${dir}/stub/log" 2>/dev/null | sed -n 3p)" "artifact-guard staged=src.txt,"
    expect_eq "H1: the checkpoint commit sets PRE_COMMIT_ALLOW_NO_CONFIG without a config" \
        "$(grep -c 'ALLOW_NO_CONFIG=1$' "${dir}/hook.log" || true)" "2"

    # H1b TDD checkpoint: the guard refuses what the round staged; not retried.
    dir="${WORK}/h1b-tdd-guard-after-round"
    checkpoint_case "${dir}"
    seed="$(git -C "${dir}/repo" rev-parse HEAD)"
    run_step "${dir}" "${STEP_TDD}" STUB_AGENT_ACTION=fix STUB_GUARD_FAIL_FROM=3
    expect_eq "H1b: exit 1" "$(rc_of "${dir}")" "1"
    expect_eq "H1b: nothing committed" "$(git -C "${dir}/repo" rev-parse HEAD)" "${seed}"
    expect_file_contains "H1b: guard refusal reported" "${dir}/step.err" "Artifact Guard blocked the files staged after repair round 1"
    expect_eq "H1b: one agent round" "$(grep -c '^hook-rejection-repair$' "${dir}/stub/calls" 2>/dev/null || true)" "1"
    expect_eq "H1b: evaluator not asked" "$(grep -c '^loop-health-evaluator$' "${dir}/stub/calls" 2>/dev/null || true)" "0"

    # H2 TDD checkpoint stuck: exit 1 with a result line, nothing committed.
    dir="${WORK}/h2-tdd-stuck"
    checkpoint_case "${dir}"
    seed="$(git -C "${dir}/repo" rev-parse HEAD)"
    run_step "${dir}" "${STEP_TDD}" STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK
    expect_eq "H2: exit 1" "$(rc_of "${dir}")" "1"
    expect_eq "H2: nothing committed" "$(git -C "${dir}/repo" rev-parse HEAD)" "${seed}"
    expect_file_contains "H2: result line on stderr" "${dir}/step.err" '"step":"checkpoint-after-implementation"'
    expect_file_contains "H2: status stuck" "${dir}/step.err" '"status":"stuck"'

    # H3 TDD checkpoint: terminal 79 passes through.
    dir="${WORK}/h3-tdd-terminal"
    checkpoint_case "${dir}"
    run_step "${dir}" "${STEP_TDD}" STUB_AGENT_RC=79
    expect_eq "H3: exit 79" "$(rc_of "${dir}")" "79"

    # H4 TDD checkpoint: malformed verdict fails the step.
    dir="${WORK}/h4-tdd-malformed"
    checkpoint_case "${dir}"
    run_step "${dir}" "${STEP_TDD}" STUB_AGENT_ACTION=noop STUB_VERDICTS=MALFORMED
    expect_eq "H4: exit 1" "$(rc_of "${dir}")" "1"

    # H5 TDD checkpoint: environment-class rejection ends as stuck.
    dir="${WORK}/h5-tdd-env"
    checkpoint_case "${dir}"
    run_step "${dir}" "${STEP_TDD}" STUB_AGENT_ACTION=noop STUB_AGENT_STATUS=not_fixable_in_code STUB_AGENT_CLASS=environment STUB_VERDICTS=STUCK
    expect_eq "H5: exit 1" "$(rc_of "${dir}")" "1"

    # H6 refactor-review checkpoint: fixed, refused, DONE.
    dir="${WORK}/h6-refactor-fixed"
    checkpoint_case "${dir}"
    run_step "${dir}" "${STEP_REFACTOR}" STUB_AGENT_ACTION=fix
    expect_eq "H6: exit 0" "$(rc_of "${dir}")" "0"
    expect_eq "H6: checkpoint message" "$(git -C "${dir}/repo" log -1 --format=%s)" "wip: checkpoint after review feedback (steps 10-11)"
    dir="${WORK}/h6-refactor-refused"
    checkpoint_case "${dir}"
    run_step "${dir}" "${STEP_REFACTOR}" STUB_AGENT_ACTION=edit-pre-commit-config
    expect_eq "H6 refused: exit 1" "$(rc_of "${dir}")" "1"
    expect_file_contains "H6 refused: hook-config-modified" "${dir}/step.err" "hook-config-modified"
    expect_eq "H6 refused: not retried" "$(hook_calls "${dir}/hook.log" pre-commit)" "1"
    dir="${WORK}/h6-refactor-done"
    checkpoint_case "${dir}"
    run_step "${dir}" "${STEP_REFACTOR}" STUB_AGENT_ACTION=noop STUB_VERDICTS=DONE
    expect_eq "H6 DONE: exit 1" "$(rc_of "${dir}")" "1"
    expect_file_contains "H6 DONE: evaluator_done_unresolved" "${dir}/step.err" "evaluator_done_unresolved"

    # H7 step-15: commit rejected, fixed, pushed; stdout is one JSON line.
    dir="${WORK}/h7-s15-commit-fixed"
    checkpoint_case "${dir}"
    install_pre_push_hook "${dir}/repo"
    run_step "${dir}" "${STEP_15}" "${STEP15_ENV[@]}" STUB_AGENT_ACTION=fix
    expect_one_stdout_line H7 "${dir}"
    expect_eq "H7: exit 0" "$(rc_of "${dir}")" "0"
    expect_eq "H7: pushed" "$(step15_field "${dir}" pushed)" "true"
    expect_eq "H7: reason" "$(step15_field "${dir}" reason)" "pushed"
    expect_eq "H7: commit title" "$(git -C "${dir}/repo" log -1 --format=%s)" "feat: add the widget"
    expect_eq "H7: committed with the fix" "$(git -C "${dir}/repo" show HEAD:src.txt 2>/dev/null)" "formatted"
    # A plain `git commit`, as before #1671: the hook never sees PRE_COMMIT_ALLOW_NO_CONFIG.
    expect_eq "H7: the commit hook ran without PRE_COMMIT_ALLOW_NO_CONFIG" \
        "$(grep '^pre-commit' "${dir}/hook.log" | grep -vc 'ALLOW_NO_CONFIG=unset$' || true)" "0"
    expect_eq "H7: the commit hook ran twice" "$(hook_calls "${dir}/hook.log" pre-commit)" "2"
    expect_eq "H7: node_modules not committed" "$(git -C "${dir}/repo" ls-tree -r --name-only HEAD | grep -c node_modules || true)" "0"
    expect_eq "H7: second artifact-guard saw the helper's staging" \
        "$(grep '^artifact-guard' "${dir}/stub/log" 2>/dev/null | sed -n 2p)" "artifact-guard staged=src.txt,"

    # H8 step-15: push rejected, fixed with a new commit, pushed.
    dir="${WORK}/h8-s15-push-fixed"
    new_repo "${dir}"
    install_pre_commit_hook "${dir}/repo"; install_pre_push_hook "${dir}/repo"
    printf 'PUSH_BAD\n' > "${dir}/repo/src.txt"
    run_step "${dir}" "${STEP_15}" "${STEP15_ENV[@]}" STUB_AGENT_ACTION=fix
    expect_one_stdout_line H8 "${dir}"
    expect_eq "H8: exit 0" "$(rc_of "${dir}")" "0"
    expect_eq "H8: pushed" "$(step15_field "${dir}" pushed)" "true"
    expect_eq "H8: fix commit on top" "$(git -C "${dir}/repo" log -1 --format=%s)" "fix: address cargo-test pre-push rejection (#1671)"
    expect_eq "H8: original commit kept beneath" "$(git -C "${dir}/repo" log -1 --format=%s HEAD~1)" "feat: add the widget"
    expect_eq "H8: sha is the pushed HEAD" "$(step15_field "${dir}" sha)" "$(git -C "${dir}/repo" rev-parse HEAD)"
    expect_eq "H8: remote matches" "$(git -C "${dir}/repo" rev-parse '@{u}')" "$(git -C "${dir}/repo" rev-parse HEAD)"
    expect_eq "H8: no commit hook saw PRE_COMMIT_ALLOW_NO_CONFIG" \
        "$(grep '^pre-commit' "${dir}/hook.log" | grep -vc 'ALLOW_NO_CONFIG=unset$' || true)" "0"

    # H9 step-15: push stuck.
    dir="${WORK}/h9-s15-push-stuck"
    new_repo "${dir}"
    install_pre_commit_hook "${dir}/repo"; install_pre_push_hook "${dir}/repo"
    printf 'PUSH_BAD\n' > "${dir}/repo/src.txt"
    run_step "${dir}" "${STEP_15}" "${STEP15_ENV[@]}" STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK
    expect_one_stdout_line H9 "${dir}"
    expect_eq "H9: exit 1" "$(rc_of "${dir}")" "1"
    expect_eq "H9: pushed false" "$(step15_field "${dir}" pushed)" "false"
    expect_eq "H9: reason" "$(step15_field "${dir}" reason)" "hook-rejected-stuck:push:cargo-test"

    # H10 step-15: commit stuck, never pushed.
    dir="${WORK}/h10-s15-commit-stuck"
    checkpoint_case "${dir}"
    install_pre_push_hook "${dir}/repo"
    run_step "${dir}" "${STEP_15}" "${STEP15_ENV[@]}" STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK
    expect_eq "H10: exit 1" "$(rc_of "${dir}")" "1"
    expect_eq "H10: reason" "$(step15_field "${dir}" reason)" "hook-rejected-stuck:commit:cargo-fmt"
    expect_eq "H10: push never attempted" "$(hook_calls "${dir}/hook.log" pre-push)" "0"

    # H11 step-15: refused on push; DONE on commit.
    dir="${WORK}/h11-s15-refused"
    new_repo "${dir}"
    install_pre_commit_hook "${dir}/repo"; install_pre_push_hook "${dir}/repo"
    printf 'PUSH_BAD\n' > "${dir}/repo/src.txt"
    run_step "${dir}" "${STEP_15}" "${STEP15_ENV[@]}" STUB_AGENT_ACTION=edit-pre-commit-config
    expect_eq "H11: reason" "$(step15_field "${dir}" reason)" "hook-rejected-refused:push:cargo-test"
    dir="${WORK}/h11-s15-done"
    checkpoint_case "${dir}"
    run_step "${dir}" "${STEP_15}" "${STEP15_ENV[@]}" STUB_AGENT_ACTION=noop STUB_VERDICTS=DONE
    expect_eq "H11 DONE: reason" "$(step15_field "${dir}" reason)" "hook-rejected-evaluator_done_unresolved:commit:cargo-fmt"

    # H12 step-15: terminal on push exits 79 with orchestration-unavailable.
    dir="${WORK}/h12-s15-terminal"
    new_repo "${dir}"
    install_pre_commit_hook "${dir}/repo"; install_pre_push_hook "${dir}/repo"
    printf 'PUSH_BAD\n' > "${dir}/repo/src.txt"
    run_step "${dir}" "${STEP_15}" "${STEP15_ENV[@]}" STUB_AGENT_RC=79
    expect_one_stdout_line H12 "${dir}"
    expect_eq "H12: exit 79" "$(rc_of "${dir}")" "79"
    expect_eq "H12: reason" "$(step15_field "${dir}" reason)" "orchestration-unavailable"
    expect_eq "H12: pushed false" "$(step15_field "${dir}" pushed)" "false"

    # H13 step-15: a commit failure that is not a hook rejection: commit-failed.
    dir="${WORK}/h13-s15-commit-failed"
    new_repo "${dir}"
    printf 'x\n' > "${dir}/repo/src.txt"
    mkdir -p "${dir}/gitbin"
    cat > "${dir}/gitbin/git" <<SHIM
#!/usr/bin/env bash
if [ "\${1:-}" = "commit" ]; then echo "fatal: simulated commit failure" >&2; exit 128; fi
exec "$(command -v git)" "\$@"
SHIM
    chmod +x "${dir}/gitbin/git"
    STEP_PATH_PREFIX="${dir}/gitbin:" run_step "${dir}" "${STEP_15}" "${STEP15_ENV[@]}"
    expect_one_stdout_line H13 "${dir}"
    expect_eq "H13: reason commit-failed" "$(step15_field "${dir}" reason)" "commit-failed"
    expect_ne "H13: non-zero exit" "$(rc_of "${dir}")" "0"
    expect_eq "H13: no agent ran" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"

    # H14 step-15: push failure that is not a hook rejection keeps push-failed.
    dir="${WORK}/h14-s15-push-failed"
    new_repo "${dir}"
    install_pre_push_hook "${dir}/repo"
    printf 'clean\n' > "${dir}/repo/src.txt"
    git -C "${dir}/repo" remote set-url --push origin "${dir}/missing.git"
    run_step "${dir}" "${STEP_15}" "${STEP15_ENV[@]}"
    expect_one_stdout_line H14 "${dir}"
    [[ "$(step15_field "${dir}" reason)" == "push-failed: "* ]] && pass \
        || fail "H14: reason must stay push-failed (got '$(step15_field "${dir}" reason)')"
    expect_eq "H14: no agent ran" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"

    # H15 step-15: diverged upstream is unchanged and never reaches the agent.
    dir="${WORK}/h15-s15-diverged"
    new_repo "${dir}"
    install_pre_push_hook "${dir}/repo"
    git clone -q "${dir}/origin.git" "${dir}/other" 2>/dev/null
    configure_identity "${dir}/other"
    git -C "${dir}/other" checkout -q "feat/issue-1671-h15-s15-diverged"
    printf 'other\n' > "${dir}/other/other.txt"
    git -C "${dir}/other" add other.txt && git -C "${dir}/other" commit -qm other && git -C "${dir}/other" push -q 2>/dev/null
    printf 'mine\n' > "${dir}/repo/src.txt"
    run_step "${dir}" "${STEP_15}" "${STEP15_ENV[@]}"
    [[ "$(step15_field "${dir}" reason)" == "diverged-upstream: "* ]] && pass \
        || fail "H15: reason must stay diverged-upstream (got '$(step15_field "${dir}" reason)')"
    expect_eq "H15: no agent ran" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"

    # H16 step-15: a commit that cannot be signed, with a passing pre-commit
    # hook installed, is commit-failed and never reaches the agent.
    dir="${WORK}/h16-s15-gpg"
    new_repo "${dir}"
    install_pre_commit_hook "${dir}/repo"
    git -C "${dir}/repo" config commit.gpgsign true
    git -C "${dir}/repo" config gpg.program false
    printf 'clean\n' > "${dir}/repo/src.txt"
    run_step "${dir}" "${STEP_15}" "${STEP15_ENV[@]}"
    expect_one_stdout_line H16 "${dir}"
    expect_eq "H16: reason commit-failed" "$(step15_field "${dir}" reason)" "commit-failed"
    expect_ne "H16: non-zero exit" "$(rc_of "${dir}")" "0"
    expect_eq "H16: no agent ran" "$(stub_calls "${dir}/stub" hook-rejection-repair)" "0"

    # H17 step-15: the agent pushes past the pre-push hook; the step refuses and
    # does not report the branch as pushed.
    dir="${WORK}/h17-s15-agent-pushes"
    new_repo "${dir}"
    install_pre_commit_hook "${dir}/repo"; install_pre_push_hook "${dir}/repo"
    printf 'PUSH_BAD\n' > "${dir}/repo/src.txt"
    run_step "${dir}" "${STEP_15}" "${STEP15_ENV[@]}" STUB_AGENT_ACTION=noop STUB_AGENT_BODY='git push -q --no-verify 2>/dev/null'
    expect_one_stdout_line H17 "${dir}"
    expect_eq "H17: exit 1" "$(rc_of "${dir}")" "1"
    expect_eq "H17: pushed false" "$(step15_field "${dir}" pushed)" "false"
    expect_eq "H17: reason" "$(step15_field "${dir}" reason)" "hook-rejected-refused:push:cargo-test"
}

# =============================================================================
# I. review findings (#1671 step 10): pipefail, global config, refused set,
#    subdirectory, snapshot failure, history limit, single print
# =============================================================================
# in_strict_shell <script>: runs <script> with both helpers sourced under
# `set -euo pipefail`, as the call sites do; prints its exit code.
in_strict_shell() {
    local rc=0
    bash -c 'set -euo pipefail; . "$1"; . "$2"; eval "$3"' _ "${STAGE_HELPER}" "${DRIVER}" "$1" >/dev/null 2>&1 || rc=$?
    printf '%s' "${rc}"
}

section_i() {
    need "${DRIVER}" I || return 0
    need "${STAGE_HELPER}" I || return 0
    local dir

    # I1 the terminal marker followed by about 1 MB of output, rc other than 79.
    dir="${WORK}/i1-terminal-marker"
    mkdir -p "${dir}"
    { echo "BLOCKED_TERMINAL orchestration_unavailable"; awk 'BEGIN { while (n < 1048576) { print "agent output line padding the log"; n += 34 } }'; echo; } > "${dir}/log"
    expect_eq "I1: marker before 1 MB of output is terminal under pipefail" \
        "$(in_strict_shell "hook_repair_terminal_refusal 1 '${dir}/log'")" "0"

    # I2 an early [rejected] line followed by more than 100 KB of remote: lines.
    dir="${WORK}/i2-push-classify"
    mkdir -p "${dir}/hooks"
    printf '#!/bin/sh\nexit 1\n' > "${dir}/hooks/pre-push"; chmod +x "${dir}/hooks/pre-push"
    {
        echo "To example.com:repo.git"
        echo " ! [rejected]        feat -> feat (fetch first)"
        awk 'BEGIN { for (i = 0; i < 4000; i++) print "remote: progress line padding the output" }'
        echo "error: failed to push some refs to 'example.com:repo.git'"
    } > "${dir}/out"
    expect_eq "I2: a rejected push is not a hook rejection under pipefail" \
        "$(in_strict_shell "hook_repair_is_rejection push 1 '${dir}/out' '${dir}/hooks'")" "1"

    # I3 the --pathspec-from-file branch runs on a git that has it.
    dir="${WORK}/i3-pathspec-from-file"
    new_repo "${dir}"
    printf 'x\n' > "${dir}/repo/new.txt"
    ( cd "${dir}/repo" && GIT_TRACE=1 bash -c 'set -euo pipefail; . "$1"; stage_workflow_changes' _ "${STAGE_HELPER}" ) \
        >/dev/null 2>"${dir}/trace" || true
    case "$(git add -h 2>&1 || true)" in
        *--pathspec-from-file*) expect_file_contains "I3: --pathspec-from-file used" "${dir}/trace" "--pathspec-from-file" ;;
        *) pass ;;
    esac
    expect_eq "I3: the file was staged" "$(git -C "${dir}/repo" diff --cached --name-only)" "new.txt"

    # I4 a global core.hooksPath change is refused.
    dir="${WORK}/i4-global-hookspath"
    commit_case "${dir}"
    mkdir -p "${dir}/empty-hooks"
    : > "${dir}/global.gitconfig"
    GIT_CONFIG_GLOBAL="${dir}/global.gitconfig" STUB_AGENT_ACTION=fix STUB_VERDICTS=STUCK \
        STUB_AGENT_BODY="git config --global core.hooksPath '${dir}/empty-hooks'" run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "I4: global core.hooksPath refused" "$(var_of "${dir}" STATUS)" "refused"
    expect_eq "I4: never retried" "$(hook_calls "${dir}/hook.log" pre-commit)" "1"

    # I7 run from a subdirectory: the snapshot still covers the top level.
    dir="${WORK}/i7-subdir"
    commit_case "${dir}"
    mkdir -p "${dir}/repo/sub/dir"
    RUN_DRIVER_SUBDIR=sub/dir STUB_AGENT_ACTION=edit-pre-commit-config STUB_VERDICTS=STUCK run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "I7: config edit refused from a subdirectory" "$(var_of "${dir}" STATUS)" "refused"
    [[ "$(var_of "${dir}" REASON)" == *".pre-commit-config.yaml"* ]] && pass \
        || fail "I7: reason must name .pre-commit-config.yaml (got '$(var_of "${dir}" REASON)')"

    # I8 a snapshot that cannot be written ends the round as stuck.
    dir="${WORK}/i8-snapshot-fails"
    commit_case "${dir}"
    STUB_AGENT_ACTION=fix STUB_VERDICTS=CONTINUE \
        STUB_AGENT_BODY='mkdir "${STUB_EVIDENCE_FILE%/*}/round-1.after"' run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "I8: status stuck" "$(var_of "${dir}" STATUS)" "stuck"
    expect_eq "I8: reason" "$(var_of "${dir}" REASON)" "refusal snapshot could not be taken"
    expect_eq "I8: never retried" "$(hook_calls "${dir}/hook.log" pre-commit)" "1"

    # I9 the evaluator receives at most 20 history lines; the full history is kept.
    dir="${WORK}/i9-history"
    commit_case "${dir}"
    STUB_AGENT_ACTION=noop STUB_VERDICTS="$(printf 'CONTINUE %.0s' {1..22})STUCK" run_driver "${dir}" "${COMMIT_ARGS[@]}"
    expect_eq "I9: stuck after 23 rounds" "$(var_of "${dir}" ROUNDS)" "23"
    expect_eq "I9: evaluator history capped at 20 lines" \
        "$(stub_arg "${dir}/stub" loop-health-evaluator 23 loop_history | grep -c '^round ' || true)" "20"
    expect_eq "I9: evaluator history ends at the current round" \
        "$(stub_arg "${dir}/stub" loop-health-evaluator 23 loop_history | awk -F: 'END { print $1 }')" "round 23"
    expect_eq "I9: full history kept in the evidence directory" \
        "$(grep -c '^round ' "$(var_of "${dir}" EVIDENCE_DIR)/history" 2>/dev/null || true)" "23"

    # I10 a failed checkpoint prints the redacted hook output exactly once.
    dir="${WORK}/i10-single-print"
    checkpoint_case "${dir}"
    run_step "${dir}" "${STEP_TDD}" STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK
    expect_eq "I10: exit 1" "$(rc_of "${dir}")" "1"
    expect_eq "I10: hook output printed once" "$(grep -c -- '- hook id: cargo-fmt' "${dir}/step.err" || true)" "1"
    dir="${WORK}/i10-single-print-refactor"
    checkpoint_case "${dir}"
    run_step "${dir}" "${STEP_REFACTOR}" STUB_AGENT_ACTION=noop STUB_VERDICTS=STUCK
    expect_eq "I10 refactor: hook output printed once" "$(grep -c -- '- hook id: cargo-fmt' "${dir}/step.err" || true)" "1"

    # I11 with nothing staged, a missing commit helper does not stop either
    # checkpoint: the helpers needed to commit load only when there is a commit.
    local step_file name
    for step_file in "${STEP_TDD}" "${STEP_REFACTOR}"; do
        name="$(basename "${step_file}")"
        dir="${WORK}/i11-nothing-staged-${name%.*}"
        new_repo "${dir}"
        RUN_STEP_HOOK_HELPER="${WORK}/no-such-helper.sh" run_step "${dir}" "${step_file}"
        expect_eq "I11 ${name}: exit 0" "$(rc_of "${dir}")" "0"
        expect_file_contains "I11 ${name}: nothing to checkpoint" "${dir}/step.out" "No staged changes to checkpoint"
    done
}

section_a
section_b
section_c
section_d
section_e
section_f
section_g
section_h_static
section_h_dynamic
section_i

echo "issue #1671 hook-rejection repair: ${PASSES} passed, ${FAILURES} failed" >&2
if (( FAILURES > 0 )); then
    exit 1
fi
echo "PASS: test-issue-1671-hook-rejection-repair"
