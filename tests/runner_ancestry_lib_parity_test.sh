#!/usr/bin/env bash
# Parity guard for the runner-ancestry shell library (issue #1593).
#
# claude-plugin/bin/install-runtime ships without scripts/, so it carries an
# inline copy of scripts/lib/runner-ancestry.sh between the lines
# '# BEGIN runner-ancestry' and '# END runner-ancestry'. This test fails when
# the two copies drift, and when either copy uses a construct POSIX sh lacks.
#
# Run: bash tests/runner_ancestry_lib_parity_test.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LIB="$REPO_ROOT/scripts/lib/runner-ancestry.sh"
RUNTIME="$REPO_ROOT/claude-plugin/bin/install-runtime"

pass=0
fail=0
TMPROOT="$(mktemp -d)"
trap 'rm -rf "$TMPROOT"' EXIT

record_pass() {
    echo "PASS: $1"
    pass=$((pass + 1))
}

record_fail() {
    echo "FAIL: $1"
    fail=$((fail + 1))
}

if [ -f "$LIB" ]; then
    record_pass "scripts/lib/runner-ancestry.sh exists"
else
    record_fail "scripts/lib/runner-ancestry.sh exists"
fi

begins="$(grep -c '^# BEGIN runner-ancestry$' "$RUNTIME")"
ends="$(grep -c '^# END runner-ancestry$' "$RUNTIME")"
if [ "$begins" = 1 ] && [ "$ends" = 1 ]; then
    record_pass "install-runtime has exactly one BEGIN/END runner-ancestry block"
else
    record_fail "install-runtime BEGIN/END markers: $begins BEGIN, $ends END (want 1 each)"
fi

# The inline copy: the lines strictly between the markers.
awk '/^# END runner-ancestry$/ { f = 0 } f { print } /^# BEGIN runner-ancestry$/ { f = 1 }' \
    "$RUNTIME" >"$TMPROOT/inline"

# The library body: the same markers inside the library, or, if the library
# carries no markers, the whole file without its shebang and leading comments.
if grep -q '^# BEGIN runner-ancestry$' "$LIB" 2>/dev/null; then
    awk '/^# END runner-ancestry$/ { f = 0 } f { print } /^# BEGIN runner-ancestry$/ { f = 1 }' \
        "$LIB" >"$TMPROOT/lib"
else
    awk 'body { print; next } /^#/ || /^[[:space:]]*$/ { next } { body = 1; print }' \
        "$LIB" >"$TMPROOT/lib" 2>/dev/null
fi

if [ -s "$TMPROOT/inline" ] && [ -s "$TMPROOT/lib" ]; then
    if diff -u "$TMPROOT/lib" "$TMPROOT/inline" >"$TMPROOT/diff"; then
        record_pass "inline copy in install-runtime matches scripts/lib/runner-ancestry.sh"
    else
        record_fail "inline copy in install-runtime has drifted from the library"
        sed 's/^/    | /' "$TMPROOT/diff"
    fi
else
    record_fail "both copies are non-empty"
fi

# Both copies define the two primitives.
for fn in runner_receipt_revision runner_relation; do
    if grep -qE "^${fn}[[:space:]]*\(\)" "$TMPROOT/inline" &&
        grep -qE "^${fn}[[:space:]]*\(\)" "$TMPROOT/lib"; then
        record_pass "both copies define $fn"
    else
        record_fail "both copies define $fn"
    fi
done

# The library is sourced by POSIX sh (install-runtime is #!/bin/sh), so it
# must parse under sh and avoid bash-only syntax.
if sh -n "$LIB" 2>"$TMPROOT/sh-n"; then
    record_pass "library parses under sh -n"
else
    record_fail "library does not parse under sh -n: $(cat "$TMPROOT/sh-n")"
fi
if grep -nE '\[\[|\$\{[A-Za-z_]+:[0-9]|<<<|\bfunction\b|=~' "$LIB" >"$TMPROOT/bashisms"; then
    record_fail "library uses bash-only syntax"
    sed 's/^/    | /' "$TMPROOT/bashisms"
else
    record_pass "library avoids bash-only syntax"
fi

# Nothing is evaluated: no eval, no sh -c, no sourcing of data.
if grep -nE '\beval\b|sh -c' "$LIB" >"$TMPROOT/eval"; then
    record_fail "library evaluates data"
    sed 's/^/    | /' "$TMPROOT/eval"
else
    record_pass "library does not eval or sh -c"
fi

# Every git call in the library uses the hardened prefix.
if grep -n 'git ' "$LIB" >"$TMPROOT/gitlines"; then
    for setting in 'credential.helper=' 'core.fsmonitor=false' 'core.hooksPath=/dev/null' \
        'protocol.allow=never' 'GIT_TERMINAL_PROMPT=0'; do
        if grep -qF -- "$setting" "$LIB"; then
            record_pass "library sets $setting"
        else
            record_fail "library does not set $setting"
        fi
    done
fi

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
