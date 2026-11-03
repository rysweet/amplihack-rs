#!/usr/bin/env bash
# Inspect loader metadata, rather than grepping arbitrary binary strings.
set -euo pipefail
if [ "$#" -eq 0 ]; then
    echo "usage: scripts/check-lbug-release-rpath.sh BINARY..." >&2
    exit 2
fi
for binary in "$@"; do
    case "$(uname -s)" in
        Darwin) metadata="$(otool -l "$binary")" ;;
        Linux) metadata="$(readelf -d "$binary")" ;;
        *) echo "unsupported linkage inspection platform" >&2; exit 2 ;;
    esac
    if printf '%s\n' "$metadata" | grep -E '(/\.cache/|/amplihack/lbug/)' >/dev/null; then
        echo "release loader metadata references a local cache: $binary" >&2
        exit 1
    fi
done
