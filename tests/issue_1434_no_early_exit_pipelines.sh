#!/usr/bin/env bash
# Guard for issue #1434 — no early-exit pipeline stage in the shell scripts CI runs.
#
# MECHANISM. `printf … | head -c N | …` — the shape that took down #1426 — leaves
# `printf` writing into a pipe that `head` has already closed. The producer dies
# of SIGPIPE, or, where SIGPIPE is IGNORED (a disposition that survives `exec`
# and is common in CI), bash's `printf` reports
#
#     bash: printf: write error: Broken pipe
#
# and returns 1. `pipefail` promotes that to the pipeline's status, the command
# substitution around it yields "", and under `set -e` the step dies. Recipe
# steps run under `set -euo pipefail`, so a script that is green here can still
# hand production a silent empty value the moment its input is large.
#
# WHY THIS IS A STATIC GUARD. Whether it fires at a given input size is a RACE on
# the 64 KB pipe buffer: the same code was green on bash 5.3.9 and red on the
# runner's 5.2.21, and an earlier 300-run attempt to reproduce a sibling failure
# came back clean because the race landed the other way every time. Execution
# cannot establish absence; the shape can be established on every machine, on
# every run. So the shape is what is asserted here — the approach #1429 took with
# its `D4-no-early-exit` check, widened from one derivation to every script the
# workflows invoke.
#
# THE FIX IS ALWAYS TO REMOVE THE EARLY EXIT, never to suppress the status: a
# shell substring (`${VAR:0:N}`), a single `awk`, or any consumer that reads all
# of its input. `|| true` converts the crash into a silently empty value, which
# is the worse half of the bug.
#
# The script list is DERIVED from `.github/workflows/*.yml`, never globbed: a
# `tests/` path glob misses the 17 scripts under `amplifier-bundle/recipes/tests/`,
# which is exactly how #1121 escaped review.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

PASS_COUNT=0
FAIL_COUNT=0
pass() { printf '  PASS[%s]: %s\n' "$1" "$2"; PASS_COUNT=$((PASS_COUNT + 1)); }
fail() { printf '  FAIL[%s]: %s\n' "$1" "$2"; FAIL_COUNT=$((FAIL_COUNT + 1)); }

# The banned shapes, assembled from fragments so this guard's own source can
# never contain a literal that it would flag.
STOPS_EARLY="$(printf '%s|%s' head tail)"
EARLY_EXIT_RE="[|][[:space:]]*(${STOPS_EARLY})([[:space:]]|\$)|grep[[:space:]]+-[[:alnum:]]*m[[:space:]]"

# scan_script <file> — print "<lineno>:<text>" for every offending line.
# Whole-line comments are skipped, as is any line carrying an explicit
# `# early-exit-ok: <reason>` justification.
scan_script() {
    awk -v re="${EARLY_EXIT_RE}" '
        /^[[:space:]]*#/   { next }
        /# early-exit-ok:/ { next }
        $0 ~ re            { printf "%d:%s\n", FNR, $0 }
    ' "$1"
}

echo "=== Issue #1434: no early-exit pipeline stage in workflow-invoked scripts ==="
echo ""

# ---------------------------------------------------------------------------
# Part G — the detector discriminates.
#
# A guard that never fires proves nothing, so it is run against fixtures with a
# known answer before it is trusted against the repository.
# ---------------------------------------------------------------------------
FIXTURES="$(mktemp -d)"
trap 'rm -rf "${FIXTURES}"' EXIT

H=head
T=tail
MFLAG=m
M="grep -${MFLAG} 1"

cat > "${FIXTURES}/bad-head.sh" <<EOF
#!/usr/bin/env bash
SLUG="\$(printf '%s' "\$TASK_DESCRIPTION" | ${H} -c 65536)"
EOF

cat > "${FIXTURES}/bad-tail.sh" <<EOF
#!/usr/bin/env bash
LAST="\$(git log --oneline | ${T} -1)"
EOF

cat > "${FIXTURES}/bad-grep-m.sh" <<EOF
#!/usr/bin/env bash
FIRST="\$(${M} needle "\$FILE")"
EOF

cat > "${FIXTURES}/safe.sh" <<'EOF'
#!/usr/bin/env bash
SLUG="${TASK_DESCRIPTION:0:65536}"
LAST="$(git log --oneline | awk 'END { print }')"
FIRST="$(awk 'n == 0 && index($0, "needle") { print; n = 1 }' "$FILE")"
COUNT="$(printf '%s' "$BLOB" | wc -c)"
EOF

cat > "${FIXTURES}/lookalikes.sh" <<EOF
#!/usr/bin/env bash
# a whole-line comment naming the banned shape: | ${H} -1
if grep -qE 'github\.(ref|${H}_ref)' <<<"\$group"; then :; fi
gh pr view --json ${H}RefName --jq '.${H}RefName'
printf '%s' "\$x" | ${H}er_is_not_a_command 2>/dev/null || true
EOF

for shape in head tail grep-m; do
    if [ -n "$(scan_script "${FIXTURES}/bad-${shape}.sh")" ]; then
        pass "G1-detects-${shape}" "the known-bad ${shape} fixture is flagged"
    else
        fail "G1-detects-${shape}" "the detector missed a known-bad ${shape} fixture — this guard proves nothing"
    fi
done

SAFE_HITS="$(scan_script "${FIXTURES}/safe.sh")"
if [ -z "${SAFE_HITS}" ]; then
    pass "G2-safe-forms-clean" "the pipefail-safe rewrites of all three shapes are not flagged"
else
    fail "G2-safe-forms-clean" "the detector flagged a safe form: ${SAFE_HITS}"
fi

LOOKALIKE_HITS="$(scan_script "${FIXTURES}/lookalikes.sh")"
if [ -z "${LOOKALIKE_HITS}" ]; then
    pass "G3-lookalikes-clean" "comments, --head flags, headRefName and (ref|head_ref) alternations are not flagged"
else
    fail "G3-lookalikes-clean" "the detector produced a false positive: ${LOOKALIKE_HITS}"
fi

# ---------------------------------------------------------------------------
# Part W — the script list, derived from the workflows themselves.
# ---------------------------------------------------------------------------
SCRIPTS="$(grep -hoE '(bash|sh) [^ ]*\.sh|scripts/[^ ]*\.sh' .github/workflows/*.yml \
           | sed -E 's/^(bash|sh) //' | sort -u)"

SCRIPT_COUNT="$(printf '%s\n' "${SCRIPTS}" | grep -c '.' || true)"
if [ "${SCRIPT_COUNT}" -ge 40 ]; then
    pass "W1-non-vacuous" "${SCRIPT_COUNT} scripts derived from .github/workflows/*.yml"
else
    fail "W1-non-vacuous" "only ${SCRIPT_COUNT} scripts derived — the derivation broke and this guard would pass vacuously"
fi

MISSING=""
while IFS= read -r s; do
    [ -n "${s}" ] || continue
    [ -f "${s}" ] || MISSING="${MISSING} ${s}"
done <<< "${SCRIPTS}"
if [ -z "${MISSING}" ]; then
    pass "W2-all-present" "every derived path exists on disk"
else
    fail "W2-all-present" "a workflow invokes a script that is not in the tree:${MISSING}"
fi

# ---------------------------------------------------------------------------
# Part E — the assertion.
# ---------------------------------------------------------------------------
OFFENDERS=""
while IFS= read -r s; do
    [ -n "${s}" ] || continue
    [ -f "${s}" ] || continue
    hits="$(scan_script "${s}")"
    [ -n "${hits}" ] || continue
    while IFS= read -r hit; do
        OFFENDERS="${OFFENDERS}
    ${s}:${hit}"
    done <<< "${hits}"
done <<< "${SCRIPTS}"

if [ -z "${OFFENDERS}" ]; then
    pass "E1-no-early-exit" "none of the ${SCRIPT_COUNT} workflow-invoked scripts has a pipeline stage that stops reading early"
else
    fail "E1-no-early-exit" "a pipeline stage stops reading before its producer is done. Under pipefail the producer's
             SIGPIPE/write-error becomes the pipeline's status and the value collapses to \"\".
             Replace it with a shell substring, a single awk, or a consumer that reads all its
             input — never with '|| true'. Offending lines:${OFFENDERS}"
fi

echo ""
echo "=== ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
[ "${FAIL_COUNT}" -eq 0 ] || exit 1
exit 0
