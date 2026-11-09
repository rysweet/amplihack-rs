#!/usr/bin/env bash
# Check that a built release binary carries the bundle manifest (issue #1972).
#
# Usage: scripts/check-embedded-bundle-manifest.sh <manifest.txt> <binary>
#
# release.yml copies the generated manifest over the empty placeholder that
# `include_str!` compiles into every binary. If that copy is skipped, lands in
# the wrong place or is overwritten, the binary embeds nothing (or something
# else), the install check is silently off, and the binary looks like a dev
# build. Run after `cargo build`, this fails the release instead.
#
# The manifest is compiled in as one string literal, so each of its lines
# appears in the binary's bytes exactly as written. Every line must be found.
# Without AMPLIHACK_RELEASE_VERSION at build time nothing can read the
# manifest, so the optimiser drops it and this check fails: it has to run on
# the binary the release ships, built the way the release builds it.
#
# Exit codes: 0 every line found, 1 a line is missing or the manifest is empty
# or has a blank line, 2 misuse. Written for bash 3.2 and POSIX tools.

set -euo pipefail
export LC_ALL=C

prog=check-embedded-bundle-manifest

if [ "$#" -ne 2 ]; then
    echo "usage: $prog <manifest.txt> <binary>" >&2
    exit 2
fi
manifest=$1
binary=$2
for file in "$manifest" "$binary"; do
    if [ ! -f "$file" ]; then
        printf '%s: not a file: %s\n' "$prog" "$file" >&2
        exit 2
    fi
done
if [ ! -s "$manifest" ]; then
    echo "$prog: the manifest is empty, so there is nothing to check" >&2
    exit 1
fi
# An empty pattern matches every line, which would hide a missing embed.
if grep -q '^$' "$manifest"; then
    echo "$prog: the manifest has a blank line" >&2
    exit 1
fi

expected=$(sort -u "$manifest" | wc -l | tr -d ' ')
found=$( { grep -a -o -F -f "$manifest" "$binary" || true; } | sort -u | wc -l | tr -d ' ')
if [ "$found" != "$expected" ]; then
    missing=$( { grep -a -o -F -f "$manifest" "$binary" || true; } | sort -u \
        | comm -13 - <(sort -u "$manifest") | sed -n 1p)
    printf '%s: %s carries %s of the %s manifest lines; first missing: %s\n' \
        "$prog" "$binary" "$found" "$expected" "$missing" >&2
    exit 1
fi
printf '%s: %s carries all %s bundle manifest lines\n' "$prog" "$binary" "$expected"
