#!/usr/bin/env bash
# test-issue-944-bugfix-no-version-bump.sh
#
# TDD contract for GitHub issue #944.
#
# Problem: the default-workflow/publish implementation commit hardcodes a
# `feat:` conventional-commit prefix (workflow-publish.yaml step-15,
# historically `printf 'feat: %.72s'`). Because step-14-bump-version treats any
# feature-signalling commit as license to bump `[workspace.package].version`,
# pure bugfix branches spuriously bump the workspace version (observed on #920
# and #943, requiring manual reverts).
#
# Contract under test:
#   R1 (behavioural) — The step-15 commit TITLE prefix is derived from the real
#       change type reported by step-14 via the env var
#       `RECIPE_VAR_version_bump__change_classification`:
#           PATCH -> "fix: "
#           MINOR -> "feat: "
#           MAJOR -> "feat!: "
#       When the classification is unset/empty, a deterministic fallback scans
#       $TASK_DESCRIPTION for bugfix intent (bug/bugfix/fix) -> "fix: ",
#       otherwise defaults to "feat: " (legacy behaviour preserved).
#   R1 (static) — The hardcoded `printf 'feat: %.72s'` literal is GONE; the
#       step-15 title logic references the classification signal and can emit
#       both `fix:` and `feat!:` prefixes.
#   R1 (safety) — Injection/word-splitting hardening from #469/#311 is
#       preserved: the task description is consumed via env var and the block
#       contains no `eval`. Since #1575 the description only chooses the
#       prefix; its text never reaches COMMIT_TITLE, which is built from the
#       staged index by workflow_commit_message.sh.
#   R2 (static) — step-14-bump-version's prompt instructs that PATCH is a
#       NO-OP on the workspace version line (leave version unchanged /
#       new_version == current_version), while MINOR/MAJOR still bump.
#
# Expected BEFORE the fix: several assertions FAIL (hardcoded feat: prefix).
# Expected AFTER  the fix: all assertions PASS.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-944-bugfix-no-version-bump.sh
# Exit codes: 0 = all pass, 1 = one or more failures, 2 = harness error.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-publish.yaml"

if [[ ! -f "${RECIPE}" ]]; then
    echo "HARNESS-ERROR: recipe not found: ${RECIPE}" >&2
    exit 2
fi
for tool in python3 jq; do
    command -v "$tool" >/dev/null 2>&1 || { echo "HARNESS-ERROR: $tool required" >&2; exit 2; }
done

WORK="$(mktemp -d -t issue944-XXXXXX)"
trap 'rm -rf "${WORK}"' EXIT

pass=0
fail=0
record_pass() { echo "PASS: $1"; pass=$((pass + 1)); }
record_fail() {
    echo "FAIL: $1" >&2
    [[ $# -gt 1 && -n "${2:-}" ]] && printf '      %s\n' "$2" >&2
    fail=$((fail + 1))
}

# ---------------------------------------------------------------------------
# Extract the two recipe fragments we exercise:
#   * step-15 commit TITLE-computation region (bounded by stable anchors)
#   * step-14-bump-version prompt text
# ---------------------------------------------------------------------------
STEP15_TITLE_SLICE="${WORK}/step15_title_slice.sh"
STEP14_PROMPT="${WORK}/step14_prompt.txt"
STEP15_FULL="${WORK}/step15_full.sh"

python3 - "$RECIPE" "$STEP15_TITLE_SLICE" "$STEP14_PROMPT" "$STEP15_FULL" <<'PY'
import sys, yaml

recipe_path, slice_out, prompt_out, full_out = sys.argv[1:5]
with open(recipe_path) as fh:
    doc = yaml.safe_load(fh)

steps = doc.get("steps") or doc.get("recipe", {}).get("steps") or []
by_id = {s.get("id"): s for s in steps if isinstance(s, dict)}

step15 = by_id.get("step-15-commit-push")
step14 = by_id.get("step-14-bump-version")
if step15 is None:
    sys.stderr.write("HARNESS-ERROR: step-15-commit-push not found\n"); sys.exit(2)
if step14 is None:
    sys.stderr.write("HARNESS-ERROR: step-14-bump-version not found\n"); sys.exit(2)

cmd = step15.get("command", "")
with open(full_out, "w") as fh:
    fh.write(cmd)

lines = cmd.splitlines()

def find(pred, default=None):
    for i, ln in enumerate(lines):
        if pred(ln):
            return i
    return default

# Start: just after the "Creating Commit" banner, else first TASK_DESC= line.
start = find(lambda l: "Creating Commit" in l)
if start is None:
    start = find(lambda l: "TASK_DESC=" in l)
    if start is None:
        sys.stderr.write("HARNESS-ERROR: could not locate commit-title region start\n"); sys.exit(2)
else:
    start += 1

# End: first Host-aware marker, else HOST_TYPE= assignment, else COMMIT_MSG=.
end = find(lambda l: "Host-aware commit references" in l)
if end is None:
    end = find(lambda l: "HOST_TYPE=" in l)
if end is None:
    end = find(lambda l: "COMMIT_MSG=" in l)
if end is None or end <= start:
    sys.stderr.write("HARNESS-ERROR: could not locate commit-title region end\n"); sys.exit(2)

with open(slice_out, "w") as fh:
    fh.write("\n".join(lines[start:end]) + "\n")

with open(prompt_out, "w") as fh:
    fh.write(step14.get("prompt", ""))
PY
rc=$?
[[ $rc -eq 0 ]] || { echo "HARNESS-ERROR: fragment extraction failed (rc=$rc)" >&2; exit 2; }

# ---------------------------------------------------------------------------
# Behavioural harness: run the real recipe title-computation slice in isolation
# and report the resulting COMMIT_TITLE for a given classification / task desc.
# Runs under `set -euo pipefail` to prove set-u safety (step-15's real shell).
# ---------------------------------------------------------------------------
# #1575: the slice calls the commit-message helper through $MSG. Run it from
# the scratch dir (not a git repo), so the helper prints nothing and the slice
# must fall back to '<prefix>update files' under `set -euo pipefail`.
MSG_HELPER="${REPO_ROOT}/amplifier-bundle/tools/workflow_commit_message.sh"
SLICE_PREAMBLE="$(printf 'cd %q\nexport MSG=%q\n' "$WORK" "$MSG_HELPER")"

compute_title() {
    # $1 = classification (empty string = unset), $2 = task description
    local class="$1" desc="$2"
    local runner="${WORK}/runner.sh"
    {
        echo 'set -euo pipefail'
        printf '%s\n' "$SLICE_PREAMBLE"
        printf 'export TASK_DESCRIPTION=%q\n' "$desc"
        printf 'export ISSUE_NUMBER=%q\n' "944"
        cat "$STEP15_TITLE_SLICE"
        echo 'printf "COMMIT_TITLE=%s\\n" "$COMMIT_TITLE"'
    } > "$runner"

    if [[ -n "$class" ]]; then
        RECIPE_VAR_version_bump__change_classification="$class" \
            bash "$runner" 2>/dev/null | sed -n 's/^COMMIT_TITLE=//p'
    else
        env -u RECIPE_VAR_version_bump__change_classification \
            bash "$runner" 2>/dev/null | sed -n 's/^COMMIT_TITLE=//p'
    fi
}

assert_prefix() {
    # $1 desc, $2 classification, $3 task, $4 expected prefix
    local title
    title="$(compute_title "$2" "$3")"
    if [[ -z "$title" ]]; then
        record_fail "$1" "slice produced no COMMIT_TITLE (likely set -u abort on unset classification var)"
    elif [[ "$title" == "$4"* ]]; then
        record_pass "$1"
    else
        record_fail "$1" "expected prefix '$4', got title: '$title'"
    fi
}

BUG_TASK="Fix GitHub issue #944: bugfix so bugfix branches do not bump version"
FEAT_TASK="Add a new publish dashboard command to the CLI"

# R1 behavioural — classification-driven prefix mapping.
assert_prefix "R1: PATCH classification -> 'fix:' prefix"   "PATCH" "$BUG_TASK"  "fix: "
assert_prefix "R1: MINOR classification -> 'feat:' prefix"  "MINOR" "$FEAT_TASK" "feat: "
assert_prefix "R1: MAJOR classification -> 'feat!:' prefix" "MAJOR" "$FEAT_TASK" "feat!: "

# R1 behavioural — deterministic fallback when classification is unset/empty.
assert_prefix "R1: unset classification + bug task -> 'fix:' (heuristic)"     "" "$BUG_TASK"  "fix: "
assert_prefix "R1: unset classification + feature task -> 'feat:' (default)"  "" "$FEAT_TASK" "feat: "

# R1 behavioural — the fix: prefix must NOT double-emit for MINOR (guard the
# mapping is a real switch, not "always fix:").
minor_title="$(compute_title "MINOR" "$BUG_TASK")"
if [[ "$minor_title" == feat:* ]]; then
    record_pass "R1: MINOR overrides bug-word heuristic (classification wins)"
else
    record_fail "R1: MINOR overrides bug-word heuristic (classification wins)" \
        "expected 'feat:' prefix from MINOR classification, got: '$minor_title'"
fi

# R1 behavioural — the uppercased alias VERSION_BUMP_CHANGE_CLASSIFICATION
# (dot -> single '_') must also drive the prefix. Guards the regression where
# a double-underscore uppercase name was used and silently never matched.
upper_title="$(
    {
        echo 'set -euo pipefail'
        printf '%s\n' "$SLICE_PREAMBLE"
        printf 'export TASK_DESCRIPTION=%q\n' "$FEAT_TASK"
        printf 'export ISSUE_NUMBER=%q\n' "944"
        cat "$STEP15_TITLE_SLICE"
        echo 'printf "COMMIT_TITLE=%s\\n" "$COMMIT_TITLE"'
    } > "${WORK}/runner_upper.sh"
    env -u RECIPE_VAR_version_bump__change_classification \
        VERSION_BUMP_CHANGE_CLASSIFICATION="PATCH" \
        bash "${WORK}/runner_upper.sh" 2>/dev/null | sed -n 's/^COMMIT_TITLE=//p'
)"
if [[ "$upper_title" == fix:* ]]; then
    record_pass "R1: uppercase alias VERSION_BUMP_CHANGE_CLASSIFICATION drives prefix"
else
    record_fail "R1: uppercase alias VERSION_BUMP_CHANGE_CLASSIFICATION drives prefix" \
        "expected 'fix:' prefix from uppercase alias, got: '$upper_title'"
fi

# R1 safety (#1575) — the task description never supplies commit-title text.
# A long marker description must leave no trace in the title.
LONG_DESC="$(printf 'q%.0s' {1..200}) trailing"
long_title="$(compute_title "PATCH" "$LONG_DESC")"
if [[ -n "$long_title" && "$long_title" != *qqqq* && "$long_title" != *trailing* ]]; then
    record_pass "R1-safety: TASK_DESC text absent from COMMIT_TITLE (#1575)"
else
    record_fail "R1-safety: TASK_DESC text absent from COMMIT_TITLE (#1575)" \
        "title='$long_title'"
fi

# R1 safety (#1575) — outside a git repo the helper prints nothing and the
# slice falls back to exactly '<prefix>update files'.
if [[ "$long_title" == "fix: update files" ]]; then
    record_pass "R1-safety: fallback title is '<prefix>update files' outside git (#1575)"
else
    record_fail "R1-safety: fallback title is '<prefix>update files' outside git (#1575)" \
        "expected 'fix: update files', got: '$long_title'"
fi

# R1 safety — an injected second line in the description must not reach the
# title, and the title stays a single physical line.
MULTILINE_DESC=$'fix newline handling\nSECOND LINE INJECTED'
ml_title="$(compute_title "PATCH" "$MULTILINE_DESC")"
if [[ -n "$ml_title" && "$ml_title" != *$'\n'* && "$ml_title" != *INJECTED* && "$ml_title" != *"newline handling"* ]]; then
    record_pass "R1-safety: single-line commit title without description text"
else
    record_fail "R1-safety: single-line commit title without description text" \
        "title: '$ml_title'"
fi

# ---------------------------------------------------------------------------
# Static assertions on the recipe source.
# ---------------------------------------------------------------------------

# R1 static — the hardcoded feat: prefix literal must be gone.
if grep -qF "printf 'feat: %.72s'" "$RECIPE"; then
    record_fail "R1-static: hardcoded \"printf 'feat: %.72s'\" removed" \
        "still present in $RECIPE (step-15)"
else
    record_pass "R1-static: hardcoded \"printf 'feat: %.72s'\" removed"
fi

# R1 static — step-15 must reference the classification signal.
if grep -qE 'version_bump__change_classification|VERSION_BUMP__?CHANGE_CLASSIFICATION' "$STEP15_FULL"; then
    record_pass "R1-static: step-15 reads version_bump change classification"
else
    record_fail "R1-static: step-15 reads version_bump change classification" \
        "no classification env var reference in step-15 command"
fi

# R1 static — step-15 must be able to emit fix: and feat!: prefixes.
if grep -qE '(^|[^a-z])fix:' "$STEP15_FULL" && grep -qE 'feat!:' "$STEP15_FULL"; then
    record_pass "R1-static: step-15 defines both 'fix:' and 'feat!:' prefixes"
else
    record_fail "R1-static: step-15 defines both 'fix:' and 'feat!:' prefixes" \
        "missing fix: and/or feat!: mapping in step-15 command"
fi

# R1 safety static — env-var read preserved; no COMMIT_TITLE line uses TASK_DESC (#1575).
if grep -qE 'TASK_DESC="\$\{?TASK_DESCRIPTION(:-\})?"?' "$STEP15_FULL" \
   && ! grep -E 'COMMIT_TITLE=' "$STEP15_FULL" | grep -q 'TASK_DESC' \
   && ! grep -qE '%\.72s' "$STEP15_FULL"; then
    record_pass "R1-safety-static: TASK_DESC from env, absent from COMMIT_TITLE (#1575)"
else
    record_fail "R1-safety-static: TASK_DESC from env, absent from COMMIT_TITLE (#1575)" \
        "TASK_DESC env assignment missing, or a COMMIT_TITLE line still uses TASK_DESC / %.72s"
fi
if grep -qE '(^|[^_[:alnum:]])eval([[:space:]]|$)' "$STEP15_FULL"; then
    record_fail "R1-safety-static: no eval in commit block" "eval found in step-15"
else
    record_pass "R1-safety-static: no eval in commit block"
fi

# R2 static — step-14 prompt makes PATCH a no-op on the workspace version.
prompt_lc="$(tr '[:upper:]' '[:lower:]' < "$STEP14_PROMPT")"
if grep -q 'patch' <<<"$prompt_lc" \
   && grep -Eq 'do not (modify|change|bump|edit)|leave .*version .*unchanged|unchanged|no-op|new_version *(==|=) *current_version|same version' <<<"$prompt_lc"; then
    record_pass "R2-static: step-14 prompt instructs PATCH is a no-op on version"
else
    record_fail "R2-static: step-14 prompt instructs PATCH is a no-op on version" \
        "prompt must tell the agent NOT to bump [workspace.package].version for PATCH"
fi

# R2 static — MINOR/MAJOR must still bump (guard against over-correction).
if grep -Eq 'minor' <<<"$prompt_lc" && grep -Eq 'major' <<<"$prompt_lc"; then
    record_pass "R2-static: step-14 prompt still distinguishes MINOR/MAJOR bumps"
else
    record_fail "R2-static: step-14 prompt still distinguishes MINOR/MAJOR bumps" \
        "prompt lost MINOR/MAJOR bump semantics"
fi

# ---------------------------------------------------------------------------
echo "---------------------------------------------"
echo "issue-944 TDD: ${pass} passed, ${fail} failed"
[[ $fail -eq 0 ]] || exit 1
exit 0
