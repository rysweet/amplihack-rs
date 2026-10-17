#!/bin/sh
# Stamp a release version into the workspace manifests (issue #1526).
# Usage, from the repository root: scripts/stamp-release-version.sh MAJOR.MINOR.PATCH
# Rewrites the [workspace.package] version in Cargo.toml, every Cargo.lock
# [[package]] with no `source` (the workspace members), and the top-level
# "version" in package.json if present. Offline and toolchain-free.
# Exit: 0 stamped or already at X; 1 validation failed (nothing written); 2 usage.
set -eu
export LC_ALL=C
SEMVER='(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'
X=${1-}
case $X in *[!0-9.]*) X=invalid ;; esac
if [ "$#" -ne 1 ] || ! printf '%s\n' "$X" | grep -Eqx "$SEMVER"; then
    echo "usage: stamp-release-version.sh MAJOR.MINOR.PATCH (got '$X')" >&2
    exit 2
fi
fail() { echo "stamp-release-version: error: $*" >&2; exit 1; }
[ -f Cargo.toml ] && [ -f Cargo.lock ] || fail "run from the repository root"
OLD=$(awk '/^\[/ { ws = ($0 == "[workspace.package]") }
    ws && /^version = "/ { v = $0; sub(/^version = "/, "", v); sub(/"$/, "", v); print v; exit }' Cargo.toml)
printf '%s\n' "$OLD" | grep -Eqx "$SEMVER" || fail "Cargo.toml [workspace.package] version '$OLD' is not MAJOR.MINOR.PATCH"
if [ "$X" = "$OLD" ]; then echo "already at $X; nothing to do"; exit 0; fi
# OLD is validated digits and dots; escape the dots for grep and sed anyway.
OLD_RE=$(printf '%s' "$OLD" | sed 's/\./\\./g')
if [ -f package.json ]; then
    grep -q "^  \"version\": \"$OLD_RE\",\{0,1\}\$" package.json || fail "package.json version is not $OLD"
fi
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
awk -v x="$X" '/^\[/ { ws = ($0 == "[workspace.package]") }
    ws && /^version = "/ { $0 = "version = \"" x "\""; n++ }
    { print } END { if (n != 1) exit 1 }' Cargo.toml >"$TMP/Cargo.toml" || fail "cannot stamp Cargo.toml"
# A [[package]] block lists `version` before `source`, so buffer each block
# and stamp it only once it is known to be a path package. Assumes every path
# package at OLD inherits the workspace version: a member with its own version
# equal to OLD would be stamped here but not in its Cargo.toml. The release
# workflow runs `cargo metadata --locked` on the result, so that case fails the
# release before the tag is pushed.
awk -v x="$X" -v old="$OLD" '
    function flush() {
        if (n && !src && ver == old) { buf[vl] = "version = \"" x "\""; c++ }
        for (i = 1; i <= n; i++) print buf[i]
        n = 0; src = 0; vl = 0; ver = ""
    }
    /^\[/ { flush() }
    { buf[++n] = $0 }
    /^version = "/ { ver = $0; sub(/^version = "/, "", ver); sub(/"$/, "", ver); vl = n }
    /^source = / { src = 1 }
    END { flush(); if (!c) exit 1 }' Cargo.lock >"$TMP/Cargo.lock" || fail "no Cargo.lock workspace member at $OLD"
if [ -f package.json ]; then
    sed "s/^  \"version\": \"$OLD_RE\"/  \"version\": \"$X\"/" package.json >"$TMP/package.json"
fi
for f in Cargo.toml Cargo.lock package.json; do
    if [ -f "$TMP/$f" ]; then cat "$TMP/$f" >"$f"; fi
done
echo "stamped $X (was $OLD)"
