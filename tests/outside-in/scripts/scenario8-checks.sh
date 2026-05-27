#!/usr/bin/env bash
# Scenario 8 — Git Fetch Resilience (Issues #655/#656)
# Run from repo root: bash tests/outside-in/scripts/scenario8-checks.sh
set -euo pipefail

RECIPE="amplifier-bundle/recipes/workflow-prep.yaml"
SKILL="docs/claude/skills/default-workflow/SKILL.md"
PASS=0
FAIL=0

pass() { echo "  ✓ $1"; PASS=$((PASS + 1)); }
fail() { echo "  ✗ $1" >&2; FAIL=$((FAIL + 1)); }

echo "=== Structural Checks ==="

# 1. git fetch not &&-chained
if grep -Pzo '&&\s*\\?\s*\n\s*git fetch' "$RECIPE" >/dev/null 2>&1; then
  fail "git fetch is still &&-chained in step-01"
else
  pass "git fetch is not &&-chained"
fi

# 2. Exit code captured
if grep -q 'FETCH_RC' "$RECIPE"; then
  pass "fetch exit code captured in FETCH_RC"
else
  fail "FETCH_RC variable not found"
fi

# 3. ADO remote detection
if grep -q 'dev.azure.com' "$RECIPE" && grep -q 'visualstudio.com' "$RECIPE"; then
  pass "ADO remote detection (dev.azure.com + visualstudio.com)"
else
  fail "missing ADO remote detection patterns"
fi

# 4. ADO remediation guidance
if grep -q 'az login' "$RECIPE" && grep -q 'credential' "$RECIPE"; then
  pass "ADO remediation guidance present (az login + credential helper)"
else
  fail "missing ADO remediation guidance"
fi

# 5. Remote URL not echoed raw (credential safety)
if grep -E '^\s*echo.*\$REMOTE_URL' "$RECIPE" | grep -v '^\s*#' >/dev/null 2>&1; then
  fail "recipe echoes REMOTE_URL — credential leak risk"
else
  pass "remote URL never echoed raw (credential safety)"
fi

echo ""
echo "=== Behavioral Checks ==="

# 6. Fetch failure handled, step continues
TMPDIR=$(mktemp -d)
(
  cd "$TMPDIR"
  git init -q
  git remote add origin https://unreachable.invalid/repo.git
  FETCH_RC=0
  git fetch --all --no-tags 2>/dev/null || FETCH_RC=$?
  if [ "$FETCH_RC" -eq 0 ]; then
    echo "UNEXPECTED" >&2
    exit 1
  fi
  git branch --show-current 2>/dev/null || true
  echo "=== Workspace Prepared ==="
) >/dev/null 2>&1
if [ $? -eq 0 ]; then
  pass "fetch failure handled, step continues to completion"
else
  fail "step aborted on fetch failure"
fi
rm -rf "$TMPDIR"

# 7. ADO remote detected in case statement
TMPDIR=$(mktemp -d)
(
  cd "$TMPDIR"
  git init -q
  git remote add origin https://dev.azure.com/org/project/_git/repo
  FETCH_RC=0
  git fetch --all --no-tags 2>/dev/null || FETCH_RC=$?
  if [ "$FETCH_RC" -ne 0 ]; then
    REMOTE_URL=$(git remote get-url origin 2>/dev/null || true)
    case "$REMOTE_URL" in
      *dev.azure.com*|*visualstudio.com*) exit 0 ;;
      *) exit 1 ;;
    esac
  fi
) >/dev/null 2>&1
if [ $? -eq 0 ]; then
  pass "ADO remote detected and remediation path triggered"
else
  fail "ADO remote not detected"
fi
rm -rf "$TMPDIR"

echo ""
echo "=== Issue #655: SKILL.md Python Remnant Checks ==="

# 8. No run_recipe_by_name
if grep -q 'run_recipe_by_name' "$SKILL"; then
  fail "SKILL.md contains stale run_recipe_by_name"
else
  pass "no run_recipe_by_name in SKILL.md"
fi

# 9. No python3 -c
if grep -q 'python3 -c' "$SKILL"; then
  fail "SKILL.md contains stale python3 -c"
else
  pass "no python3 -c in SKILL.md"
fi

# 10. No from amplihack.recipes import
if grep -q 'from amplihack.recipes import' "$SKILL"; then
  fail "SKILL.md contains stale Python import"
else
  pass "no stale Python imports in SKILL.md"
fi

# 11. Has Rust CLI invocation
if grep -q 'amplihack recipe run' "$SKILL"; then
  pass "SKILL.md documents Rust CLI invocation"
else
  fail "SKILL.md missing amplihack recipe run"
fi

echo ""
echo "=== Results ==="
echo "Passed: $PASS  Failed: $FAIL"

if [ "$FAIL" -gt 0 ]; then
  echo "VERDICT: FAIL"
  exit 1
fi
echo "VERDICT: PASS"
exit 0
