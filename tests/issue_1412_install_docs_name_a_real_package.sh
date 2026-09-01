#!/usr/bin/env bash
# Issue #1412: every documented `cargo install --path` must name a directory
# that actually contains an installable binary package.
#
# Two commands were wrong, and both failed for different reasons:
#
#   cargo install --path .                     -> "found a virtual manifest at
#                                                 .../Cargo.toml instead of a
#                                                 package manifest"
#   cargo install --path crates/amplihack-cli  -> "there is nothing to install
#                                                 ... because it has no binaries"
#
# The second could never have worked: amplihack-cli is a library crate. A new
# user following the docs hits a setup failure that says nothing about what to
# do instead.
#
# This asserts the property rather than a literal path, so it keeps working if
# the binary package moves.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

pass=0
fail=0
record_pass() { printf 'PASS: %s\n' "$1"; pass=$((pass + 1)); }
record_fail() { printf 'FAIL: %s\n' "$1"; [ $# -gt 1 ] && printf '      %s\n' "$2"; fail=$((fail + 1)); }

# Every `cargo install --path <dir>` in the docs, excluding placeholders.
while IFS= read -r line; do
    file="${line%%:*}"
    path="$(printf '%s' "$line" | sed -n 's/.*cargo install --path \([^ `]*\).*/\1/p')"
    case "$path" in
        ""|'<'*) continue ;;   # placeholder like <binary-package-dir>
    esac

    manifest="$REPO_ROOT/$path/Cargo.toml"
    if [ "$path" = "." ]; then
        record_fail "$file names an installable package (got '.')" \
            "the repository root is a virtual workspace manifest; 'cargo install --path .' cannot work"
        continue
    fi
    if [ ! -f "$manifest" ]; then
        record_fail "$file names a directory with a Cargo.toml (got '$path')" "no manifest at $manifest"
        continue
    fi
    # A package is installable only if it produces a binary: an explicit
    # [[bin]], or a src/main.rs.
    if grep -q '^\[\[bin\]\]' "$manifest" || [ -f "$REPO_ROOT/$path/src/main.rs" ]; then
        record_pass "$file installs from '$path', which produces a binary"
    else
        record_fail "$file installs from '$path', which produces a binary" \
            "$path has no [[bin]] and no src/main.rs -- 'cargo install' refuses libraries"
    fi
done < <(grep -rn 'cargo install --path' docs/ README.md 2>/dev/null)

if [ "$((pass + fail))" -eq 0 ]; then
    record_fail "found documented install commands to check" \
        "no 'cargo install --path' found in docs/ or README.md -- this guard would pass vacuously"
fi

printf '\n=== Results: %d passed, %d failed ===\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
