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
exit 0
