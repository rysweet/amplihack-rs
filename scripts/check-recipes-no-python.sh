#!/usr/bin/env bash
# check-recipes-no-python.sh — static guard for issue #248
#
# Validates that no shipped recipe in amplifier-bundle/recipes/ executes a
# bare `python -m` or `python3 -m` invocation that would hard-fail in a
# Python-free environment.
#
# Allowed patterns (these do NOT count as violations):
#   - Comment lines (start with optional whitespace then `#`)
#   - Probe loops (`for cand in python3 python; do ...`)
#   - Substituted invocations after a probe (`"$PY" -m ...`)
#   - `python -m pytest` (test invocation, out of scope per #248)
#
# Forbidden:
#   - `python -m <module>` or `python3 -m <module>` directly
#   - `PYTHONPATH=<...> python -m <module>` directly
#
# Exits 0 if the recipes are clean; exits 1 (with locations) if a regression
# is introduced. Wire this into CI to lock in the work landed in PRs
# #327, #328, #329, #330, #347, #348.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RECIPES_DIR="${REPO_ROOT}/amplifier-bundle/recipes"

if [[ ! -d "${RECIPES_DIR}" ]]; then
    echo "ERROR: ${RECIPES_DIR} not found" >&2
    exit 2
fi

violations=0
violations_log=""

# Match `python -m` or `python3 -m`, optionally preceded by `PYTHONPATH=<word>`.
# Use word boundaries on both sides so we don't match `"$PY"` or `pythonish`.
forbidden_re='(^|[^"$[:alnum:]_])(PYTHONPATH=[^[:space:]]+[[:space:]]+)?python3?[[:space:]]+-m[[:space:]]+'

while IFS= read -r -d '' file; do
    # grep -n with -E gives line numbers; we then filter by allowed patterns.
    while IFS=: read -r lineno content; do
        # Skip comment lines (optional whitespace, then `#`)
        if [[ "${content}" =~ ^[[:space:]]*# ]]; then
            continue
        fi
        # Skip pytest invocations (out of scope for #248)
        if [[ "${content}" =~ python3?[[:space:]]+-m[[:space:]]+pytest ]]; then
            continue
        fi
        # Real violation
        violations=$((violations + 1))
        violations_log+="  ${file#${REPO_ROOT}/}:${lineno}: ${content}"$'\n'
    done < <(grep -nE "${forbidden_re}" "${file}" || true)
done < <(find "${RECIPES_DIR}" -maxdepth 2 -type f -name '*.yaml' -print0)

if [[ ${violations} -gt 0 ]]; then
    cat >&2 <<EOF
FAIL: ${violations} bare python invocation(s) found in shipped recipes.

This guard exists to lock in the work landed in PRs #327, #328, #329, #330,
#347, #348 (issue #248) — recipes shipped to users must not hard-fail when
Python is absent.

Use the graceful-skip pattern instead:

  for cand in python3 python; do
      if command -v "\$cand" >/dev/null 2>&1; then
          if PYTHONPATH=src "\$cand" -c 'import <module>' >/dev/null 2>&1; then
              PY="\$cand"; break
          fi
      fi
  done
  if [ -z "\${PY:-}" ]; then
      echo '[skip] python <module> unavailable' >&2
      echo '{"skipped":true}'
      exit 0
  fi
  PYTHONPATH=src "\$PY" -m <module> ...

Violations:
${violations_log}
EOF
    exit 1
fi

echo "PASS: amplifier-bundle/recipes/ contains no bare python -m invocations."

# ============================================================================
# Second pass (issue #495): detect any reintroduction of the legacy
# `build_publish_validation_<suffix>.py` helper inside shipped bundle dirs.
#
# Scope (deliberate): amplifier-bundle/{recipes,agents,prompts}/ only.
# Excluded: docs/, .git/, target/, node_modules/ — historical references in
# docs/ are allowed (they describe the prior Python implementation).
# Pinned regex matches: build_publish_validation_<word>.py
# ============================================================================
val_violations=0
val_log=""
val_re='build_publish_validation_[A-Za-z0-9_]+\.py'

for sub in recipes agents prompts; do
    scan_dir="${REPO_ROOT}/amplifier-bundle/${sub}"
    if [[ ! -d "${scan_dir}" ]]; then
        continue
    fi
    while IFS= read -r -d '' file; do
        # Skip the recipes/tests/ regression-test directory — those files
        # legitimately reference build_publish_validation_*.py as fixtures
        # and as documentation of the contract being tested.
        case "${file}" in
            "${REPO_ROOT}/amplifier-bundle/recipes/tests/"*) continue ;;
        esac
        while IFS=: read -r lineno content; do
            val_violations=$((val_violations + 1))
            val_log+="  ${file#${REPO_ROOT}/}:${lineno}: ${content}"$'\n'
        done < <(grep -nE -- "${val_re}" "${file}" 2>/dev/null || true)
    done < <(find "${scan_dir}" -type f \( -name '*.yaml' -o -name '*.yml' -o -name '*.md' -o -name '*.sh' -o -name '*.txt' \) -print0)
done

if [[ ${val_violations} -gt 0 ]]; then
    cat >&2 <<EOF
FAIL: ${val_violations} reference(s) to build_publish_validation_*.py found in
shipped amplifier-bundle/{recipes,agents,prompts}/.

This guard exists to lock in issue #495: the Python helper
build_publish_validation_*.py is not shipped in amplihack-rs and any reference
strands the recipe under a Python-free environment. Use the warn-and-continue
shell pattern documented in workflow-publish.yaml step-15 instead.

Violations:
${val_log}
EOF
    exit 1
fi

echo "PASS: amplifier-bundle/{recipes,agents,prompts}/ contains no build_publish_validation_*.py references."
exit 0
