#!/usr/bin/env bash
# Extract an Office Open XML archive (.docx/.pptx/.xlsx) into a directory.

set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: unpack.sh <office_file> <output_directory>" >&2
    exit 2
fi

input=$(realpath "$1")
output=$(realpath -m "$2")

if [[ ! -f "$input" ]]; then
    echo "unpack.sh: input file not found: $input" >&2
    exit 1
fi

rm -rf "$output"
mkdir -p "$output"
if command -v unzip >/dev/null 2>&1; then
    unzip -q "$input" -d "$output"
elif command -v 7z >/dev/null 2>&1; then
    7z x -bb0 -bso0 -bsp0 "-o${output}" "$input" >/dev/null
elif command -v jar >/dev/null 2>&1; then
    (
        cd "$output"
        jar xf "$input"
    )
else
    echo "unpack.sh: requires one archive tool: unzip, 7z, or jar" >&2
    exit 1
fi
