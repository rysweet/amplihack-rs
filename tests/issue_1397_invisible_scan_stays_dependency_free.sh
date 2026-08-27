#!/usr/bin/env bash
# Issue #1397: the external-PR invisible-character scan must build in seconds.
#
# The scanner reads stdin and writes a verdict. It used to live in the
# `amplihack` package, so running it compiled that package's entire dependency
# graph -- 196 crates -- and the job hit its 15-minute timeout, which reads as
# a failed required check on every external PR.
#
# It now lives in its own crate with no dependencies. This guard fails if
# anything reintroduces one, or if the workflow goes back to building it out of
# a package that has them.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="$REPO_ROOT/bins/scan-invisible-chars/Cargo.toml"
WORKFLOW="$REPO_ROOT/.github/workflows/invisible-char-scan.yml"

pass=0
fail=0
record_pass() { printf 'PASS: %s\n' "$1"; pass=$((pass + 1)); }
record_fail() { printf 'FAIL: %s\n' "$1"; [ $# -gt 1 ] && printf '      %s\n' "$2"; fail=$((fail + 1)); }
assert() { if eval "$2" >/dev/null 2>&1; then record_pass "$1"; else record_fail "$1" "condition: $2"; fi; }

assert "the scanner has its own manifest" "[ -f '$MANIFEST' ]"
assert "the scanner has its own source" "[ -f '$REPO_ROOT/bins/scan-invisible-chars/src/main.rs' ]"
assert "the scanner is a workspace member" \
    "grep -q 'bins/scan-invisible-chars' '$REPO_ROOT/Cargo.toml'"

# The whole point: an empty [dependencies] table.
if [ -f "$MANIFEST" ]; then
    deps="$(awk '/^\[dependencies\]/{f=1;next} /^\[/{f=0} f && NF && $0 !~ /^#/' "$MANIFEST")"
    if [ -z "$deps" ]; then
        record_pass "the scanner declares no dependencies"
    else
        record_fail "the scanner declares no dependencies" "found: $(printf '%s' "$deps" | tr '\n' ' ')"
    fi
fi

assert "the scanner source pulls in no workspace crate" \
    "! grep -qE '^use +(amplihack|anyhow|clap|serde|tracing)' '$REPO_ROOT/bins/scan-invisible-chars/src/main.rs'"

assert "the workflow builds the standalone crate, not the amplihack package" \
    "grep -q 'cargo run --locked -p scan-invisible-chars-bin' '$WORKFLOW'"
assert "the workflow no longer builds -p amplihack" \
    "! grep -q 'cargo run.*-p amplihack ' '$WORKFLOW'"

assert "no stale [[bin]] for the scanner remains in bins/amplihack" \
    "! grep -q 'scan-invisible-chars' '$REPO_ROOT/bins/amplihack/Cargo.toml'"

printf '\n=== Results: %d passed, %d failed ===\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
