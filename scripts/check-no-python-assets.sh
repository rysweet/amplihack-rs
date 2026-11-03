#!/usr/bin/env bash
# check-no-python-assets.sh — fail if Python implementation assets are tracked.

set -euo pipefail

violations="$(
    while IFS= read -r file; do
        [[ -e "${file}" ]] || continue
        printf '%s\n' "${file}"
    done < <(git ls-files | grep -Ei '(^|/)([^/]+\.py|pyproject\.toml|requirements[^/]*\.txt|setup\.py|setup\.cfg|Pipfile|poetry\.lock|uv\.lock)$' | grep -Ev '^(pyproject\.toml|tests/test_issue_1917_lbug_[a-z_]+\.py)$' || true)
)"

if [[ -n "${violations}" ]]; then
    cat >&2 <<EOF
FAIL: tracked Python implementation/package assets found.

amplihack-rs must not ship Python source or Python package metadata beyond the
root pyproject.toml used only for the uvx shell bootstrap and the isolated
issue-1917 test harnesses explicitly required for local build-cache validation.
Those tests are not packaged runtime/build assets. Keep language
references, fixtures, and Python-project detection in Rust code/docs, but do
not add executable Python assets back to this repository.

Violations:
${violations}
EOF
    exit 1
fi

echo "PASS: no tracked Python source or package metadata assets."
