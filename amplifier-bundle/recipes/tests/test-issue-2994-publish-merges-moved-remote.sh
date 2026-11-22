#!/usr/bin/env bash
# test-issue-2994-publish-merges-moved-remote.sh: the publishing and feedback
# steps merge a remote branch that moved, and never rebase it (issue #2994).
#
# Cases (S = step-18c, 20b, 16b, consensus step9, consensus step12):
#   T0   harness proof: the recording pre-push hook fires on a known-good push
#   18c-A   remote diverged, no conflict: a two-parent merge, one pull, one
#           push, the hook fired
#   18c-B1  remote has a conflicting change: the step fails without its marker,
#           git's CONFLICT text is on stderr, the branch is checked out, no
#           merge or rebase is in progress; a stand-in agent resolves it,
#           commits and pushes
#   18c-B2  as B1, but the stand-in agent aborts and returns CHANGE
#   18c-C   the #2839 report is dirty and the remote does not touch it: the
#           merge succeeds, the report's bytes and refs/stash are unchanged
#   18c-D   pre-push refuses: one pull, one push, the hook's message on stderr
#   20b, 16b, step9, step12: a moved remote merges and pushes; a conflict
#           reports git's message, is not reported as pushed and leaves the
#           branch checked out; step9 and step12 redact credentials in git's
#           output; a refused push is one push
#   S1   workflow_pull_merge never discards a merge the caller already began
#   ARGV the recorded git commands hold no rebase, force, hook bypass,
#        `+` refspec or core.hooksPath
#
# What is real: git, a bare origin, a linked worktree per case, a recording
# pre-push hook in the client's hooks directory, and the bash of each step
# taken from its recipe and run as the runner would run it. The scratch global
# git configuration sets pull.rebase=true and pull.ff=only, the settings under
# which a plain `git pull` would rebase or refuse.
#
# What is a stand-in: `amplihack` (the artifact guard only) and the recovery
# agent, which runs the fetch and merge commands read from
# push-failure-recovery.yaml's own prompt. The gate is covered by
# tests/integration/push_failure_recovery.rs. Assertions use git state, hook
# logs, exit codes, each step's marker or JSON output, and git's stderr.
#
# This test SHOULD FAIL before #2994 is implemented and MUST PASS afterwards.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-2994-publish-merges-moved-remote.sh
# Exit codes: 0 = pass, 1 = fail, 2 = harness error.

set -euo pipefail

# An enclosing hook's repository selectors override even an absolute `git
# init <fixture>` target (#1937). Isolate before the first fixture Git call.
unset GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_CEILING_DIRECTORIES GIT_COMMON_DIR \
    GIT_DIR GIT_DISCOVERY_ACROSS_FILESYSTEM GIT_INDEX_FILE GIT_INDEX_VERSION \
    GIT_NAMESPACE GIT_OBJECT_DIRECTORY GIT_PREFIX GIT_QUARANTINE_PATH GIT_WORK_TREE \
    GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT GH_REPO ISSUE_NUMBER REPO_PATH \
    WORKFLOW_STAGE_CHANGES_HELPER RECIPE_VAR_step_recovery_dir
unset AMPLIHACK_GIT_AUTHOR_NAME AMPLIHACK_GIT_AUTHOR_EMAIL \
    AMPLIHACK_GIT_COMMITTER_NAME AMPLIHACK_GIT_COMMITTER_EMAIL AMPLIHACK_CONFIG
while IFS= read -r git_variable; do
    case "${git_variable}" in GIT_CONFIG_KEY_*|GIT_CONFIG_VALUE_*) unset "${git_variable}" ;; esac
done < <(compgen -e)
unset git_variable

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
STAGE_HELPER="${REPO_ROOT}/amplifier-bundle/tools/workflow_stage_changes.sh"

for tool in git python3 jq; do
    command -v "${tool}" >/dev/null 2>&1 || { echo "HARNESS-ERROR: ${tool} is required" >&2; exit 2; }
done
python3 -I -c 'import yaml' 2>/dev/null || { echo "HARNESS-ERROR: python3 needs the yaml module" >&2; exit 2; }
for path in "${RECIPES}/workflow-pr-review.yaml" "${RECIPES}/workflow-finalize.yaml" \
    "${RECIPES}/workflow-publish.yaml" "${RECIPES}/consensus-publish.yaml" \
    "${RECIPES}/consensus-pr-feedback.yaml" "${RECIPES}/push-failure-recovery.yaml" "${STAGE_HELPER}"; do
    [[ -f "${path}" ]] || { echo "HARNESS-ERROR: ${path} not found" >&2; exit 2; }
done
REAL_GIT="$(command -v git)"
rg() { "${REAL_GIT}" "$@"; }   # fixture and assertion git: never recorded

SCRATCH_PARENT="${TMPDIR:-}"
[[ "${SCRATCH_PARENT}" == /* && -d "${SCRATCH_PARENT}" ]] || { echo "HARNESS-ERROR: TMPDIR must be an existing absolute directory" >&2; exit 2; }
SCRATCH_PARENT="$(cd -- "${SCRATCH_PARENT:?}" && pwd -P)"
WORK="$(mktemp -d "${SCRATCH_PARENT:?}/issue-2994-merge-XXXXXX")" || exit 2
case "${WORK}" in "${SCRATCH_PARENT:?}/issue-2994-merge-"*) ;; *) echo "HARNESS-ERROR: invalid scratch allocation" >&2; exit 2 ;; esac
[[ -n "${ISSUE_2994_KEEP_WORK:-}" ]] || trap 'rm -rf -- "${WORK:?}"' EXIT
chmod 700 "${WORK}"
export HOME="${WORK}/home" XDG_CONFIG_HOME="${WORK}/home/.config"
export GIT_CONFIG_GLOBAL="${WORK}/home/.gitconfig" GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0
export GIT_MERGE_AUTOEDIT=no
BIN="${WORK}/bin"
mkdir -p "${HOME}" "${BIN}"

FAILURES=0
PASSES=0
fail() { echo "FAIL: $*" >&2; FAILURES=$((FAILURES + 1)); }
pass() { PASSES=$((PASSES + 1)); }
expect_eq() { if [[ "$2" == "$3" ]]; then pass; else fail "$1: expected '$3', got '$2'"; fi; }
expect_nonempty() { if [[ -s "$2" ]]; then pass; else fail "$1: ${2##*/} is empty or missing"; fi; }
expect_has() { if grep -qF -- "$3" "$2"; then pass; else fail "$1: '$3' not found in ${2##*/}"; fi; }
expect_lacks() { if grep -qF -- "$3" "$2"; then fail "$1: '$3' found in ${2##*/}"; else pass; fi; }

# --- stand-ins on PATH -------------------------------------------------------
# git wrapper: record argv (all runs, and the current run), then run real git.
cat > "${BIN}/git" <<WRAP
#!/usr/bin/env bash
line="git"; for a in "\$@"; do line+=" \$(printf '%q' "\$a")"; done
printf '%s\n' "\$line" >> "${WORK}/git-argv.all"
printf '%s\n' "\$line" >> "${WORK}/git-argv.run"
exec "${REAL_GIT}" "\$@"
WRAP
cat > "${BIN}/amplihack" <<'STUB'
#!/usr/bin/env bash
case "$1 ${2:-}" in "hygiene artifact-guard") exit 0 ;; esac
echo "stub amplihack: unexpected call: $*" >&2
exit 64
STUB
chmod +x "${BIN}/git" "${BIN}/amplihack"
: > "${WORK}/git-argv.all"; : > "${WORK}/git-argv.run"
export PATH="${BIN}:${PATH}"
export AMPLIHACK_HOME="${REPO_ROOT}" AMPLIHACK_BIN="${BIN}/amplihack"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com

rg config --global init.defaultBranch main
rg config --global user.name t
rg config --global user.email t@example.com
rg config --global push.default upstream
rg config --global advice.pushUpdateRejected false
# The settings under which a plain `git pull` rebases, or refuses a merge.
rg config --global pull.rebase true
rg config --global pull.ff only

# --- the real step text, and the recovery prompt's own commands --------------
python3 -I - "${RECIPES}" "${WORK}" <<'PY'
import re, sys, yaml
recipes, work = sys.argv[1:3]
def steps(name):
    return {s["id"]: s for s in yaml.safe_load(open(f"{recipes}/{name}.yaml"))["steps"]}
def put(name, text):
    open(f"{work}/{name}", "w").write(text)
put("step-18c.sh", steps("workflow-pr-review")["step-18c-push-feedback-changes"]["command"])
put("step-20b.sh", steps("workflow-finalize")["step-20b-push-cleanup"]["command"])
put("step-9.sh", steps("consensus-publish")["step9-commit"]["command"])
put("step-12.sh", steps("consensus-pr-feedback")["step12-push-updates"]["command"])
prompt = steps("workflow-publish")["step-16b-outside-in-fix-loop"]["prompt"]
blocks = [b for b in re.findall(r"```bash\n(.*?)```", prompt, re.S) if "git commit -m" in b]
if len(blocks) != 1:
    sys.exit("step-16b: expected one bash block that commits, found %d" % len(blocks))
put("step-16b.sh", blocks[0])
rec = yaml.safe_load(open(f"{recipes}/push-failure-recovery.yaml"))["steps"]
text = next(s["prompt"] for s in rec if s.get("prompt") and "Remote has commits HEAD lacks" in s["prompt"])
put("recovery-prompt.txt", text)
PY

# --- fixture ---------------------------------------------------------------
ORIGIN="${WORK}/origin.git"
MAIN="${WORK}/main"
rg init -q --bare "${ORIGIN}"
rg clone -q "${ORIGIN}" "${MAIN}" 2>/dev/null
REPORT="run/reports/report.json"
( cd "${MAIN}" && mkdir -p run/reports && echo base > base.txt && echo base > shared.txt \
    && echo '{"report":"v1"}' > "${REPORT}" && rg add -A && rg commit -q -m base && rg push -q origin main )

HOOK_DIR="$(cd "${MAIN}" && cd "$(rg rev-parse --git-path hooks)" && pwd)"
HOOK_LOG="${WORK}/pre-push.log"
REFUSE_MARKER="${WORK}/refuse-push"
# The recording hook drains stdin, logs, and refuses while the marker exists.
# When it refuses it also prints a URL carrying a credential, as a hook that
# echoes its remote might.
cat > "${HOOK_DIR}/pre-push" <<HOOK
#!/usr/bin/env bash
cat >/dev/null
echo "pre-push fired" >> "${HOOK_LOG}"
if [ -e "${REFUSE_MARKER}" ]; then
  echo "HOOK-REFUSED: marker file present (https://user:SECRET2994@example.invalid/x)" >&2
  exit 1
fi
exit 0
HOOK
chmod +x "${HOOK_DIR}/pre-push"

new_case() { # <name> -> CASE_BRANCH CASE_WT; the worktree tracks origin/<branch>
    CASE_BRANCH="pr-$1"
    CASE_WT="${WORK}/wt-$1"
    ( cd "${MAIN}" && rg worktree add -q -b "${CASE_BRANCH}" "${CASE_WT}" main )
    ( cd "${CASE_WT}" && rg push -q -u origin "${CASE_BRANCH}" 2>/dev/null )
}
remote_head() { rg -C "${ORIGIN}" rev-parse "refs/heads/$1"; }
wt_head() { rg -C "$1" rev-parse HEAD; }
move_remote() { # <name> <file> <content>: another actor pushes to the branch
    local other="${WORK}/other-$1"
    rg clone -q "${ORIGIN}" "${other}" 2>/dev/null
    ( cd "${other}" && rg checkout -q "pr-$1" && printf '%s\n' "$3" > "$2" && rg add "$2" \
        && rg commit -q -m "another actor's commit" && rg push -q origin "pr-$1" )
}
prep_local() { # <site> <wt> <file> <content>: the change the step publishes
    case "$1" in
        18c|20b) ( cd "$2" && printf '%s\n' "$4" > "$3" && rg add "$3" && rg commit -q -m "run's own commit" ) ;;
        *)       printf '%s\n' "$4" > "$2/$3" ;;   # the step commits it
    esac
}

# Run one step's bash in the worktree as the runner would. Sets RC OUT ERR and
# leaves the git commands of this run in ${WORK}/git-argv.run.
RC=0; OUT=""; ERR=""
run_site() { # <site> <wt> <label>
    local site="$1" wt="$2" label="$3" script
    case "${site}" in
        18c) script="${WORK}/step-18c.sh" ;;
        20b) script="${WORK}/step-20b.sh" ;;
        16b) script="${WORK}/step-16b.sh" ;;
        s9)  script="${WORK}/step-9.sh" ;;
        s12) script="${WORK}/step-12.sh" ;;
        *) echo "HARNESS-ERROR: unknown site ${site}" >&2; exit 2 ;;
    esac
    OUT="${WORK}/${label}.out"; ERR="${WORK}/${label}.err"
    : > "${WORK}/git-argv.run"; : > "${HOOK_LOG}"
    RC=0
    ( cd "${wt}" && \
      RECIPE_VAR_worktree_setup="$(jq -nc --arg p "${wt}" '{worktree_path: $p}')" \
      RECIPE_VAR_task_description="issue 2994 fixture" RECIPE_VAR_is_critical_code=false \
      ISSUE_NUMBER=2994 bash "${script}" ) >"${OUT}" 2>"${ERR}" || RC=$?
}
count_cmd() { grep -cE "^git( .*)? $1( |\$)" "${WORK}/git-argv.run" || true; }
parents() { rg -C "$1" rev-list --parents -n1 HEAD; }
marker_for() { case "$1" in 18c) echo "=== Changes Pushed ===" ;; 20b) echo "=== Cleanup Complete ===" ;; *) echo "" ;; esac; }
json_field() { tail -n1 "${OUT}" | jq -r ".$1" 2>/dev/null || echo "unparsable"; }
no_merge_in_progress() { # <label> <wt>
    if rg -C "$2" rev-parse -q --verify MERGE_HEAD >/dev/null 2>&1; then fail "$1: MERGE_HEAD is left behind"; else pass; fi
    local d
    for d in rebase-merge rebase-apply; do
        if [[ -e "$2/$(rg -C "$2" rev-parse --git-path "${d}")" || -e "$(rg -C "$2" rev-parse --git-path "${d}")" ]]; then fail "$1: a ${d} directory exists"; else pass; fi
    done
}
branch_checked_out() { expect_eq "$1 branch is checked out, HEAD not detached" "$(rg -C "$2" symbolic-ref -q --short HEAD || echo DETACHED)" "$3"; }
stash_ref() { rg -C "$1" rev-parse -q --verify refs/stash 2>/dev/null || echo none; }

# --- T0: the recording hook fires on a known-good push ----------------------
new_case t0
( cd "${CASE_WT}" && echo ok > ok.txt && rg add ok.txt && rg commit -q -m ok && : > "${HOOK_LOG}" && rg push -q )
expect_nonempty "T0 recording pre-push hook fired on a known-good push" "${HOOK_LOG}"
expect_eq "T0 remote carries the pushed commit" "$(remote_head "${CASE_BRANCH}")" "$(wt_head "${CASE_WT}")"

# --- the moved-remote case, for every site ----------------------------------
moved_case() { # <site>
    local site="$1" marker pre tip
    new_case "m-${site}"
    prep_local "${site}" "${CASE_WT}" local.txt local
    move_remote "m-${site}" other.txt theirs
    tip="$(remote_head "${CASE_BRANCH}")"
    run_site "${site}" "${CASE_WT}" "moved-${site}"
    expect_eq "${site} moved: exit status" "${RC}" "0"
    marker="$(marker_for "${site}")"
    [[ -z "${marker}" ]] || expect_has "${site} moved: marker" "${OUT}" "${marker}"
    expect_eq "${site} moved: merge commit has two parents" "$(parents "${CASE_WT}" | wc -w)" "3"
    expect_eq "${site} moved: second parent is the remote tip before the step" \
        "$(parents "${CASE_WT}" | cut -d' ' -f3)" "${tip}"
    expect_eq "${site} moved: the push landed" "$(remote_head "${CASE_BRANCH}")" "$(wt_head "${CASE_WT}")"
    expect_nonempty "${site} moved: the push went through the hook" "${HOOK_LOG}"
    expect_eq "${site} moved: one pull" "$(count_cmd pull)" "1"
    expect_eq "${site} moved: one push" "$(count_cmd push)" "1"
    expect_lacks "${site} moved: no rebase wording on stderr" "${ERR}" "pull --rebase"
    no_merge_in_progress "${site} moved" "${CASE_WT}"
    branch_checked_out "${site} moved" "${CASE_WT}" "${CASE_BRANCH}"
    case "${site}" in
        s9)  expect_eq "s9 moved: pushed" "$(json_field pushed)" "true"
             expect_eq "s9 moved: push_reason" "$(json_field push_reason)" "success" ;;
        s12) expect_eq "s12 moved: updates_pushed" "$(json_field updates_pushed)" "true"
             expect_eq "s12 moved: push_reason" "$(json_field push_reason)" "success" ;;
    esac
    if [[ "${site}" == "18c" ]]; then
        pre="$(rg -C "${CASE_WT}" rev-parse HEAD^1)"
        expect_eq "18c-A HEAD^1 is the step's own pre-merge commit" \
            "$(rg -C "${CASE_WT}" log -1 --format=%s "${pre}")" "run's own commit"
    fi
}

# --- the conflict case, for every site --------------------------------------
# Leaves CONFLICT_WT, CONFLICT_BRANCH, CONFLICT_TIP, CONFLICT_PRE for the agent.
conflict_case() { # <site> <name>
    local site="$1" name="$2"
    new_case "${name}"
    CONFLICT_WT="${CASE_WT}"; CONFLICT_BRANCH="${CASE_BRANCH}"
    prep_local "${site}" "${CASE_WT}" shared.txt mine
    move_remote "${name}" shared.txt theirs
    CONFLICT_TIP="$(remote_head "${CASE_BRANCH}")"
    run_site "${site}" "${CASE_WT}" "${name}"
    CONFLICT_PRE="$(wt_head "${CASE_WT}")"
    expect_has "${site} conflict: git's CONFLICT text reaches stderr" "${ERR}" "CONFLICT"
    expect_lacks "${site} conflict: no rebase wording on stderr" "${ERR}" "pull --rebase"
    expect_eq "${site} conflict: nothing was pushed" "$(remote_head "${CASE_BRANCH}")" "${CONFLICT_TIP}"
    branch_checked_out "${site} conflict" "${CASE_WT}" "${CASE_BRANCH}"
    no_merge_in_progress "${site} conflict" "${CASE_WT}"
    expect_eq "${site} conflict: tracked status is clean" \
        "$(rg -C "${CASE_WT}" status --porcelain --untracked-files=no)" ""
    expect_eq "${site} conflict: one pull" "$(count_cmd pull)" "1"
    expect_eq "${site} conflict: no push after a failed merge" "$(count_cmd push)" "0"
    case "${site}" in
        18c|20b|16b)
            if [[ "${RC}" -ne 0 ]]; then pass; else fail "${site} conflict: expected a non-zero exit"; fi
            local marker; marker="$(marker_for "${site}")"
            [[ -z "${marker}" ]] || expect_lacks "${site} conflict: no completion marker" "${OUT}" "${marker}" ;;
        s9)  expect_eq "s9 conflict: exit status" "${RC}" "0"
             expect_eq "s9 conflict: pushed" "$(json_field pushed)" "false"
             expect_eq "s9 conflict: push_reason" "$(json_field push_reason)" "push_failed" ;;
        s12) expect_eq "s12 conflict: exit status" "${RC}" "0"
             expect_eq "s12 conflict: updates_pushed" "$(json_field updates_pushed)" "false"
             expect_eq "s12 conflict: push_reason" "$(json_field push_reason)" "push_failed" ;;
    esac
}

# --- the refused case --------------------------------------------------------
refused_case() { # <site>
    local site="$1" marker
    new_case "r-${site}"
    prep_local "${site}" "${CASE_WT}" local.txt local
    : > "${REFUSE_MARKER}"
    run_site "${site}" "${CASE_WT}" "refused-${site}"
    rm -f "${REFUSE_MARKER}"
    expect_eq "${site} refused: one pull" "$(count_cmd pull)" "1"
    expect_eq "${site} refused: one push" "$(count_cmd push)" "1"
    expect_has "${site} refused: the hook's message reaches stderr" "${ERR}" "HOOK-REFUSED"
    expect_lacks "${site} refused: no rebase wording on stderr" "${ERR}" "pull --rebase"
    branch_checked_out "${site} refused" "${CASE_WT}" "${CASE_BRANCH}"
    no_merge_in_progress "${site} refused" "${CASE_WT}"
    marker="$(marker_for "${site}")"
    case "${site}" in
        18c|20b|16b)
            if [[ "${RC}" -ne 0 ]]; then pass; else fail "${site} refused: expected a non-zero exit"; fi
            [[ -z "${marker}" ]] || expect_lacks "${site} refused: no completion marker" "${OUT}" "${marker}" ;;
        s9)  expect_eq "s9 refused: pushed" "$(json_field pushed)" "false"
             expect_eq "s9 refused: push_reason" "$(json_field push_reason)" "push_failed" ;;
        s12) expect_eq "s12 refused: updates_pushed" "$(json_field updates_pushed)" "false"
             expect_eq "s12 refused: push_reason" "$(json_field push_reason)" "push_failed" ;;
    esac
    case "${site}" in
        s9|s12|20b)   # steps that redact credentials in git's output
            expect_has "${site} refused: URL userinfo is redacted" "${ERR}" "REDACTED@example.invalid"
            expect_lacks "${site} refused: no credential on stderr" "${ERR}" "SECRET2994" ;;
    esac
    expect_eq "${site} refused: the remote did not move" \
        "$(remote_head "${CASE_BRANCH}")" "$(rg -C "${ORIGIN}" rev-parse "refs/heads/main")"
}

for site in 18c 20b 16b s9 s12; do moved_case "${site}"; done

# --- 18c-B1 / 18c-B2: conflict, then the stand-in recovery agent -------------
# The agent reads the fetch and merge commands from push-failure-recovery.yaml's
# prompt, substitutes the branch, and runs each as an argument array.
agent_commands() { # <wt> <branch> <log>: run the prompt's fetch and merge
    local wt="$1" branch="$2" log="$3" line word
    local -a words cmd
    : > "${log}"
    while IFS= read -r line; do
        read -r -a words <<<"${line}"
        [[ "${words[0]:-}" == env && "${words[6]:-}" == git ]] || continue
        case "${words[7]:-}" in fetch|merge) ;; *) continue ;; esac
        [[ "${words[7]}" == fetch && "${line}" == *"git fetch origin" ]] \
            || [[ "${words[7]}" == merge && "${line}" == *"git merge --no-edit origin/<branch_name>" ]] || continue
        cmd=(env GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never)
        for word in "${words[@]:6}"; do cmd+=("${word//<branch_name>/${branch}}"); done
        ( cd "${wt}" && "${cmd[@]}" ) >>"${log}" 2>&1 || true
    done < "${WORK}/recovery-prompt.txt"
}

conflict_case 18c b1
AGENT_LOG="${WORK}/agent-b1.log"
agent_commands "${CONFLICT_WT}" "${CONFLICT_BRANCH}" "${AGENT_LOG}"
expect_has "18c-B1 the agent sees git's CONFLICT text" "${AGENT_LOG}" "CONFLICT"
( cd "${CONFLICT_WT}" && printf 'resolved\n' > shared.txt && rg add shared.txt \
    && GIT_EDITOR=true rg commit -q --no-edit && rg push -q ) >>"${AGENT_LOG}" 2>&1 || fail "18c-B1 the agent could not resolve and push: $(tail -n3 "${AGENT_LOG}")"
expect_eq "18c-B1 the resolved merge reached the remote" "$(remote_head "${CONFLICT_BRANCH}")" "$(wt_head "${CONFLICT_WT}")"
expect_eq "18c-B1 the resolution has two parents" "$(parents "${CONFLICT_WT}" | wc -w)" "3"
expect_eq "18c-B1 the remote tip is the second parent" "$(parents "${CONFLICT_WT}" | cut -d' ' -f3)" "${CONFLICT_TIP}"

conflict_case 18c b2
AGENT_LOG="${WORK}/agent-b2.log"
agent_commands "${CONFLICT_WT}" "${CONFLICT_BRANCH}" "${AGENT_LOG}"
expect_has "18c-B2 the agent sees git's CONFLICT text" "${AGENT_LOG}" "CONFLICT"
( cd "${CONFLICT_WT}" && rg merge --abort ) >>"${AGENT_LOG}" 2>&1 || fail "18c-B2 merge --abort failed"
echo CHANGE > "${WORK}/agent-b2.cause"
expect_eq "18c-B2 the agent returned CHANGE" "$(cat "${WORK}/agent-b2.cause")" "CHANGE"
branch_checked_out "18c-B2" "${CONFLICT_WT}" "${CONFLICT_BRANCH}"
no_merge_in_progress "18c-B2 after the abort" "${CONFLICT_WT}"
expect_eq "18c-B2 HEAD is the run's own commit" "$(wt_head "${CONFLICT_WT}")" "${CONFLICT_PRE}"
expect_eq "18c-B2 the remote is as the other actor left it" "$(remote_head "${CONFLICT_BRANCH}")" "${CONFLICT_TIP}"

# --- 18c-C: the #2839 report is dirty and the remote does not touch it -------
new_case c
prep_local 18c "${CASE_WT}" local.txt local
move_remote c other.txt theirs
printf '{"report":"v2 rewritten by the run"}\n' > "${CASE_WT}/${REPORT}"
REPORT_BYTES="$(cat "${CASE_WT}/${REPORT}")"
STASH_BEFORE="$(stash_ref "${CASE_WT}")"
run_site 18c "${CASE_WT}" "report-18c"
expect_eq "18c-C exit status" "${RC}" "0"
expect_has "18c-C marker" "${OUT}" "=== Changes Pushed ==="
expect_eq "18c-C the merge has two parents" "$(parents "${CASE_WT}" | wc -w)" "3"
expect_eq "18c-C the report's bytes are intact" "$(cat "${CASE_WT}/${REPORT}")" "${REPORT_BYTES}"
expect_eq "18c-C refs/stash is unchanged" "$(stash_ref "${CASE_WT}")" "${STASH_BEFORE}"
expect_eq "18c-C the push landed" "$(remote_head "${CASE_BRANCH}")" "$(wt_head "${CASE_WT}")"
rg -C "${CASE_WT}" show -s HEAD >/dev/null
if rg -C "${CASE_WT}" ls-tree -r --name-only HEAD | grep -qx "${REPORT}"; then pass; else fail "18c-C the tracked report is missing from HEAD"; fi
if [[ "$(rg -C "${CASE_WT}" show "HEAD:${REPORT}")" == "${REPORT_BYTES}" ]]; then
    fail "18c-C the rewritten report was committed (#2839)"; else pass; fi

# --- conflicts and refusals at every site ------------------------------------
for site in 20b 16b s9 s12; do conflict_case "${site}" "x-${site}"; done
conflict_case 18c x-18c
for site in 18c 20b 16b s9 s12; do refused_case "${site}"; done

# --- S1: a merge the caller already began is never discarded -----------------
new_case s1
prep_local 18c "${CASE_WT}" shared.txt mine
move_remote s1 shared.txt theirs
( cd "${CASE_WT}" && rg fetch -q origin && ( rg merge --no-edit "origin/${CASE_BRANCH}" >/dev/null 2>&1 || true ) )
S1_MERGE_HEAD="$(rg -C "${CASE_WT}" rev-parse -q --verify MERGE_HEAD || echo missing)"
S1_BYTES="$(cat "${CASE_WT}/shared.txt")"
if [[ "${S1_MERGE_HEAD}" == "missing" ]]; then fail "S1 fixture: the manual merge did not conflict"; else pass; fi
S1_RC=0
( cd "${CASE_WT}" && . "${STAGE_HELPER}" && workflow_pull_merge ) >"${WORK}/s1.out" 2>"${WORK}/s1.err" || S1_RC=$?
if [[ "${S1_RC}" -ne 0 ]]; then pass; else fail "S1 workflow_pull_merge succeeded over an unresolved merge"; fi
expect_eq "S1 MERGE_HEAD is unchanged" "$(rg -C "${CASE_WT}" rev-parse -q --verify MERGE_HEAD || echo missing)" "${S1_MERGE_HEAD}"
if [[ -n "$(rg -C "${CASE_WT}" ls-files -u)" ]]; then pass; else fail "S1 the unmerged index entries were discarded"; fi
expect_eq "S1 the conflicted file keeps its bytes" "$(cat "${CASE_WT}/shared.txt")" "${S1_BYTES}"
( cd "${CASE_WT}" && rg merge --abort ) >/dev/null 2>&1 || true

# --- ARGV: nothing rewrote history, forced a push or skipped the hooks -------
expect_nonempty "recorded git commands (the wrapper was on PATH)" "${WORK}/git-argv.all"
python3 -I - "${WORK}/git-argv.all" <<'PY' || fail "a recorded git command rebased, forced, skipped hooks or set core.hooksPath"
import shlex, sys
bad = []
for raw in open(sys.argv[1]):
    raw = raw.rstrip("\n")
    try:
        argv = shlex.split(raw)
    except ValueError:
        argv = raw.split()
    args = argv[1:]
    sub, i = None, 0
    while i < len(args):
        a = args[i]
        if a in ("-c", "-C"):
            if a == "-c" and i + 1 < len(args):
                key = args[i + 1].split("=", 1)[0].lower()
                if key.startswith("pull.rebase") or key == "core.hookspath":
                    bad.append(raw)
            i += 2
            continue
        if a.startswith("-"):
            i += 1
            continue
        sub = a.lower()
        break
    rest = args[i + 1:] if sub else []
    if sub == "rebase":
        bad.append(raw)
    if sub == "pull" and any(a in ("--rebase", "-r") or a.startswith("--rebase=") for a in rest):
        bad.append(raw)
    if sub == "push" and any(
        a in ("--force", "--force-with-lease", "--force-if-includes", "-f", "--delete", "-d", "--mirror", "--prune")
        or a.startswith(("--force-with-lease=", "+", ":"))
        for a in rest
    ):
        bad.append(raw)
    if sub == "commit" and any(a in ("-n", "--amend") for a in rest):
        bad.append(raw)
    if sub == "reset" and "--hard" in rest:
        bad.append(raw)
    if "--no-verify" in args:
        bad.append(raw)
if bad:
    for line in sorted(set(bad)):
        print("forbidden: " + line, file=sys.stderr)
    sys.exit(1)
PY

echo "passes=${PASSES} failures=${FAILURES}"
[[ "${FAILURES}" -eq 0 ]] || exit 1
exit 0
