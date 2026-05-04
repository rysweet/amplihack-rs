#!/usr/bin/env bash
# Repackage an extracted Office Open XML directory into an Office file.

set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
    echo "usage: pack.sh <input_directory> <office_file> [--force]" >&2
    exit 2
fi

input=$(realpath "$1")
output=$(realpath -m "$2")
force=${3:-}

if [[ "$force" != "" && "$force" != "--force" ]]; then
    echo "pack.sh: unsupported option: $force" >&2
    exit 2
fi

if [[ ! -d "$input" ]]; then
    echo "pack.sh: input directory not found: $input" >&2
    exit 1
fi

if [[ -e "$output" && "$force" != "--force" ]]; then
    echo "pack.sh: output already exists: $output (use --force to overwrite)" >&2
    exit 1
fi

output_dir=$(dirname "$output")
mkdir -p "$output_dir"
tmp=$(mktemp "${output}.tmp.XXXXXX")
rm -f "$tmp"

if command -v zip >/dev/null 2>&1; then
    (
        cd "$input"
        zip -qr "$tmp" .
    )
elif command -v 7z >/dev/null 2>&1; then
    (
        cd "$input"
        7z a -tzip -bb0 -bso0 -bsp0 "$tmp" . >/dev/null
    )
elif command -v jar >/dev/null 2>&1; then
    jar cf "$tmp" -C "$input" .
else
    echo "pack.sh: requires one archive tool: zip, 7z, or jar" >&2
    exit 1
fi

mv "$tmp" "$output"
