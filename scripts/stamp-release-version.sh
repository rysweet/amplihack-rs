#!/usr/bin/env bash
# Stamp a release version into the workspace manifests (issue #1526).
#
# Usage: scripts/stamp-release-version.sh <MAJOR.MINOR.PATCH>
#
# Run from the repository root. Rewrites, in the current directory:
#   - Cargo.toml    the `version` line of [workspace.package]
#   - Cargo.lock    the `version` line of every [[package]] without a `source`
#                   (the workspace members)
#   - package.json  the top-level "version"
#
# The Cargo.lock edit is what `cargo update --workspace --offline` produces
# after the Cargo.toml edit; doing it as text means the Auto Release `release`
# job needs no Rust toolchain. Nothing else is touched.
#
# Exit status:
#   0  stamped, or already at the requested version (no file changed)
#   1  validation failed; no file was modified
#   2  usage error: not exactly one argument, or not MAJOR.MINOR.PATCH
#
# Every check runs before any file is written. Plain bash and POSIX awk only:
# the release job runs on a bare runner, and BSD/mawk/gawk must all agree.

set -euo pipefail
export LC_ALL=C

PROG=stamp-release-version
# MAJOR.MINOR.PATCH, ASCII digits, no leading zeros, no pre-release or build.
SEMVER_RE='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
FILES=(Cargo.toml Cargo.lock package.json)

usage() {
    printf 'usage: %s <MAJOR.MINOR.PATCH>\n' "$PROG" >&2
    exit 2
}

fail() {
    printf '%s: error: %s\n' "$PROG" "$*" >&2
    exit 1
}

if [[ $# -ne 1 ]]; then
    printf '%s: error: expected exactly one argument, got %d\n' "$PROG" "$#" >&2
    usage
fi
X=$1
if ! [[ $X =~ $SEMVER_RE ]]; then
    printf '%s: error: %q is not MAJOR.MINOR.PATCH (no "v" prefix, suffix or leading zeros)\n' \
        "$PROG" "$X" >&2
    usage
fi

for f in "${FILES[@]}"; do
    [[ -f $f && -r $f ]] || fail "$f: not found or not readable in the current directory"
    [[ -s $f ]] || fail "$f: is empty"
    if [[ -e $f.stamp-tmp || -L $f.stamp-tmp ]]; then
        fail "$f.stamp-tmp already exists; remove it after checking where it came from"
    fi
    if grep -q $'\r' "$f"; then
        fail "$f: contains a carriage return; refusing to rewrite CRLF line endings"
    fi
    [[ -z $(tail -c 1 "$f") ]] || fail "$f: does not end with a newline"
done

# Each pass validates its file and, with mode=check, prints what the caller
# needs (the old version, or the number of path packages). With mode=write it
# re-runs the same checks and prints the stamped file instead, so validation
# and rewriting cannot disagree. Versions arrive through -v and are compared
# as strings, never used as regular expressions.

toml_pass() {
    awk -v mode="$1" -v x="$X" -v prog="$PROG" '
        function die(msg) {
            print prog ": error: Cargo.toml: " msg | "cat 1>&2"
            failed = 1
            exit 1
        }
        {
            line = $0
            if (line ~ /^[ \t]*\[/) {
                section = line
                sub(/^[ \t]+/, "", section)
                sub(/[ \t]+$/, "", section)
                in_ws = (section == "[workspace.package]")
            }
            if (in_ws && line ~ /^[ \t]*version[ \t]*=/) {
                if (line !~ /^version = "[^"]*"$/)
                    die("unrecognised version line in [workspace.package]: " line)
                n++
                old = line
                sub(/^version = "/, "", old)
                sub(/"$/, "", old)
                if (mode == "write")
                    line = "version = \"" x "\""
            }
            if (mode == "write")
                print line
        }
        END {
            if (failed)
                exit 1
            if (n != 1)
                die("[workspace.package] must have exactly one version line; found " n)
            if (mode == "check")
                print old
        }
    ' Cargo.toml
}

lock_pass() {
    # Two passes over Cargo.lock: the first finds the [[package]] blocks with
    # no `source` (a block lists `version` before `source`, so a block is only
    # known to be a path package once it ends); the second checks dependency
    # entries against them and, in write mode, prints the stamped file.
    awk -v mode="$1" -v x="$X" -v old="$OLD" -v prog="$PROG" '
        function die(msg) {
            print prog ": error: Cargo.lock: " msg | "cat 1>&2"
            failed = 1
            exit 1
        }
        function close_block() {
            if (in_block && !has_source) {
                if (name == "" || version_line == 0)
                    die("[[package]] at line " start " has no name or version line")
                if (version != old)
                    die("unexpected path package " name " at " version " (expected " old ")")
                if (name in path_package)
                    die("duplicate path package " name)
                path_package[name] = 1
                stamp_line[version_line] = 1
                count++
            }
            in_block = 0
        }
        FNR == NR {
            if ($0 == "[[package]]") {
                close_block()
                in_block = 1
                start = FNR
                name = ""
                version = ""
                version_line = 0
                has_source = 0
                next
            }
            if ($0 ~ /^\[/) {
                close_block()
                next
            }
            if (!in_block)
                next
            if ($0 ~ /^name = "/) {
                name = $0
                sub(/^name = "/, "", name)
                sub(/"$/, "", name)
            } else if ($0 ~ /^version = "/) {
                version = $0
                sub(/^version = "/, "", version)
                sub(/"$/, "", version)
                version_line = FNR
            } else if ($0 ~ /^source = /) {
                has_source = 1
            }
            next
        }
        FNR == 1 {
            close_block()
            second_pass = 1
            if (count == 0)
                die("no [[package]] without a source; expected the workspace members")
        }
        {
            # A dependency entry carries a version only when two packages share
            # a name. For a path package that version would need stamping too,
            # which this script does not do: refuse instead of guessing.
            if ($0 ~ /^ "/) {
                dep = $0
                sub(/^ "/, "", dep)
                sub(/",?$/, "", dep)
                if (split(dep, part, " ") >= 2 && (part[1] in path_package))
                    die("dependency \"" dep "\" names path package " part[1] " with a version")
            }
            if (mode == "write")
                print ((FNR in stamp_line) ? "version = \"" x "\"" : $0)
        }
        END {
            if (failed)
                exit 1
            if (!second_pass)
                die("could not read the file twice")
            if (mode == "check")
                print count
        }
    ' Cargo.lock Cargo.lock
}

json_pass() {
    awk -v mode="$1" -v x="$X" -v old="$OLD" -v prog="$PROG" '
        function die(msg) {
            print prog ": error: package.json: " msg | "cat 1>&2"
            failed = 1
            exit 1
        }
        {
            line = $0
            if (line ~ /^  "version"[ \t]*:/) {
                if (line !~ /^  "version": "[^"]*",?$/)
                    die("unrecognised top-level version line: " line)
                n++
                v = line
                sub(/^  "version": "/, "", v)
                sub(/",?$/, "", v)
                if (v != old)
                    die("top-level version is " v ", but Cargo.toml [workspace.package] says " old)
                if (mode == "write") {
                    comma = (line ~ /,$/) ? "," : ""
                    line = "  \"version\": \"" x "\"" comma
                }
            }
            if (mode == "write")
                print line
        }
        END {
            if (failed)
                exit 1
            if (n != 1)
                die("expected exactly one top-level \"version\" line; found " n)
        }
    ' package.json
}

OLD=$(toml_pass check)
[[ $OLD =~ $SEMVER_RE ]] ||
    fail "Cargo.toml: [workspace.package] version $(printf '%q' "$OLD") is not MAJOR.MINOR.PATCH"
COUNT=$(lock_pass check)
json_pass check

if [[ $X == "$OLD" ]]; then
    printf 'already at %s; nothing to do\n' "$X"
    exit 0
fi

# Write phase. The leftover check above guarantees no temp file exists yet;
# noclobber makes creating one fail rather than follow anything planted since,
# and the trap removes only the files this run created.
created=()
cleanup() {
    local f
    for f in ${created[@]+"${created[@]}"}; do
        rm -f -- "$f"
    done
}
trap cleanup EXIT
set -C

stage() {
    local tmp="$1.stamp-tmp"
    : >"$tmp" || fail "cannot create $tmp"
    created+=("$tmp")
    "$2" write >>"$tmp"
}

stage Cargo.toml toml_pass
stage Cargo.lock lock_pass
stage package.json json_pass

for f in "${FILES[@]}"; do
    mv -f -- "$f.stamp-tmp" "$f"
done

printf 'stamped %s (was %s): Cargo.toml 1, Cargo.lock %s, package.json 1\n' "$X" "$OLD" "$COUNT"
