#!/usr/bin/env bash
# Write the release manifest of an amplifier-bundle directory (issue #1972).
#
# Usage: scripts/generate-bundle-manifest.sh <amplifier-bundle-dir> > manifest.txt
#
# Prints one line per file, `<sha256>  <path>`, paths relative to the directory
# and sorted by byte value. This is the format `sha256sum -c` reads and the one
# the binary parses (bundle_manifest::parse). The file set is the one install
# copies (filesystem::walk_tree with WalkScope::CopySet):
#   - symlinks and special files are not listed and never followed;
#   - directories named __pycache__, .pytest_cache or node_modules are skipped;
#   - files whose names end in .pyc or .pyo are skipped.
# A parity test (issue_1972_generator.rs) runs this script against the Rust walk
# on every PR, so the two cannot drift apart unnoticed.
#
# Every file is hashed in one pass, and each hasher line is checked against
# the exact `<digest>  ./<path>` line expected for that file.
#
# Exit codes: 0 success, 1 rejected input (a path the format cannot carry, an
# empty tree, or a hashing failure), 2 misuse. Nothing is written to stdout
# unless the whole manifest was built. Written for bash 3.2 and POSIX tools.

set -euo pipefail
export LC_ALL=C

prog=generate-bundle-manifest

if [ "$#" -ne 1 ]; then
    echo "usage: $prog <amplifier-bundle-dir>" >&2
    exit 2
fi
root=$1
if [ -L "$root" ] || [ ! -d "$root" ]; then
    printf '%s: not a directory, or a symlink: %q\n' "$prog" "$root" >&2
    exit 2
fi

if command -v sha256sum >/dev/null 2>&1; then
    hasher="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
    hasher="shasum -a 256"
else
    echo "$prog: neither sha256sum nor shasum is available" >&2
    exit 2
fi

list=$(mktemp)
sorted=$(mktemp)
hashes=$(mktemp)
out=$(mktemp)
trap 'rm -f -- "$list" "$sorted" "$hashes" "$out"' EXIT

cd -- "$root"

find -P . \
    \( -type d \( -name __pycache__ -o -name .pytest_cache -o -name node_modules \) -prune \) \
    -o \( -type f ! -name '*.pyc' ! -name '*.pyo' -print0 \) >"$list"
sort -z <"$list" >"$sorted"

reject() {
    printf '%s: %s: %q\n' "$prog" "$1" "$2" >&2
    exit 1
}

# Every path must be valid UTF-8. One pass over the whole list, and a second
# pass only to name the offending path.
if ! iconv -f UTF-8 -t UTF-8 <"$sorted" >/dev/null 2>&1; then
    while IFS= read -r -d '' path; do
        if ! printf '%s' "$path" | iconv -f UTF-8 -t UTF-8 >/dev/null 2>&1; then
            reject "path is not valid UTF-8" "${path#./}"
        fi
    done <"$sorted"
    echo "$prog: the file list is not valid UTF-8" >&2
    exit 1
fi

replacement=$(printf '\357\277\275')
count=0
while IFS= read -r -d '' path; do
    rel=${path#./}
    case $rel in
    *[[:cntrl:]]*) reject "control character in path" "$rel" ;;
    *\\*) reject "backslash in path" "$rel" ;;
    \**) reject "path starts with *" "$rel" ;;
    *"$replacement"*) reject "U+FFFD in path" "$rel" ;;
    esac
    count=$((count + 1))
done <"$sorted"

if [ "$count" -eq 0 ]; then
    printf '%s: no regular files under %q\n' "$prog" "$root" >&2
    exit 1
fi

# One hasher run over every file, in list order. The paths were checked above
# for the characters a hasher escapes (backslash, newline), and each output
# line is compared with the exact line expected below, so neither escaping nor
# a mis-pairing can reach the manifest.
if ! xargs -0 $hasher -- <"$sorted" >"$hashes"; then
    echo "$prog: hashing failed" >&2
    exit 1
fi

exec 3<"$hashes"
while IFS= read -r -d '' path; do
    rel=${path#./}
    if ! IFS= read -r line <&3; then
        reject "hasher printed no line for" "$rel"
    fi
    digest=${line%% *}
    case $digest in
    *[!0-9a-f]* | "") reject "hasher printed an unexpected digest for" "$rel" ;;
    esac
    if [ "${#digest}" -ne 64 ] || [ "$line" != "$digest  $path" ]; then
        reject "hasher printed an unexpected line for" "$rel"
    fi
    printf '%s  %s\n' "$digest" "$rel" >>"$out"
done <"$sorted"
if IFS= read -r line <&3; then
    echo "$prog: hasher printed more lines than there are files" >&2
    exit 1
fi
exec 3<&-

cat -- "$out"
