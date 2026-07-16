#!/usr/bin/env bash
# check-toolchain-refs.sh — enforce the toolchain-ref invariant behind issue #935.
#
# Root cause of #935: rust-toolchain.toml pins channel = "1.97.0", but a build
# workflow used `dtolnay/rust-toolchain@stable` together with
# `targets: ${{ matrix.target }}`. dtolnay@stable adds the cross target's
# rust-std to the STABLE toolchain, but at build time rust-toolchain.toml
# overrides resolution back to 1.97.0 — which never had that target's std added
# — producing `error[E0463]: can't find crate for std` on cross-compile legs.
#
# Invariant enforced here:
#   Every `dtolnay/rust-toolchain@<ref>` step that ALSO declares a `targets:`
#   key (i.e. it provisions a cross-target's rust-std) MUST pin <ref> to the
#   channel declared in rust-toolchain.toml. Steps with no `targets:` key
#   (scan / native-only jobs) may stay on a floating ref such as `@stable`.
#
# Usage:
#   scripts/check-toolchain-refs.sh                 # scan all .github/workflows/*.yml
#   scripts/check-toolchain-refs.sh path/to/wf.yml  # scan explicit files
#
# Exit status:
#   0  no non-allowlisted drift found
#   1  at least one drifted targets-bearing ref (or missing prerequisites)
#
# Environment overrides:
#   TOOLCHAIN_TOML  path to the rust-toolchain.toml providing the channel
#                   (defaults to the repo-root rust-toolchain.toml).
#
# See docs/reference/ci-pipeline.md.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TOOLCHAIN_TOML="${TOOLCHAIN_TOML:-$REPO_ROOT/rust-toolchain.toml}"

# Allowlist of workflow basenames with a KNOWN, tracked drift of this class that
# is out of scope for the current change. Allowlisted drift is reported as a
# visible WARNING (never silently ignored) but does not fail the check. Remove
# an entry once the underlying workflow is pinned so stale allowlist entries are
# surfaced too.
#
# The allowlist is currently empty: every targets-bearing toolchain ref
# (including release.yml, pinned in #939) must match the rust-toolchain.toml
# channel. Add an entry only to track a genuinely out-of-scope, in-progress
# pinning follow-up. See docs/reference/ci-pipeline.md.
ALLOWLIST=()

err() { printf 'ERROR: %s\n' "$1" >&2; }
warn() { printf 'WARNING: %s\n' "$1" >&2; }

# Resolve the pinned channel (e.g. 1.97.0) from rust-toolchain.toml.
toolchain_channel() {
    sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' \
        "$TOOLCHAIN_TOML" | head -n1
}

# Emit "lineno:ref" for every dtolnay/rust-toolchain step in $1 that declares a
# `targets:` key within the few lines following the step (before the next step
# or toolchain action).
targets_bearing_refs() {
    local file="$1"
    awk '
        /dtolnay\/rust-toolchain@/ {
            ref = $0
            sub(/.*dtolnay\/rust-toolchain@/, "", ref)
            sub(/[[:space:]].*/, "", ref)
            ln = FNR
            hit = 0
            for (i = 1; i <= 6; i++) {
                if ((getline line) <= 0) break
                if (line ~ /^[[:space:]]*targets:/) { hit = 1; break }
                if (line ~ /dtolnay\/rust-toolchain@/) break
            }
            if (hit) print ln ":" ref
        }
    ' "$file"
}

is_allowlisted() {
    local base="$1" entry
    # Expanding "${ALLOWLIST[@]}" on an empty array under `set -u` is an
    # "unbound variable" error on bash < 4.4 (e.g. macOS system bash 3.2).
    # Return early so an empty allowlist stays robust across bash versions.
    [ "${#ALLOWLIST[@]}" -eq 0 ] && return 1
    for entry in "${ALLOWLIST[@]}"; do
        [ "$base" = "$entry" ] && return 0
    done
    return 1
}

main() {
    if [ ! -f "$TOOLCHAIN_TOML" ]; then
        err "rust-toolchain.toml not found at: $TOOLCHAIN_TOML"
        return 1
    fi

    local channel
    channel="$(toolchain_channel)"
    if [ -z "$channel" ]; then
        err "could not resolve [toolchain] channel from: $TOOLCHAIN_TOML"
        return 1
    fi

    # Collect the files to scan: explicit args, else every workflow file.
    local files=()
    if [ "$#" -gt 0 ]; then
        files=("$@")
    else
        local wf
        for wf in "$REPO_ROOT"/.github/workflows/*.yml \
                  "$REPO_ROOT"/.github/workflows/*.yaml; do
            [ -f "$wf" ] && files+=("$wf")
        done
    fi

    if [ "${#files[@]}" -eq 0 ]; then
        warn "no workflow files to check"
        return 0
    fi

    echo "check-toolchain-refs: enforcing dtolnay/rust-toolchain@$channel on targets-bearing steps"

    local violations=0
    local allowlisted=0
    local file base entry ref lineno

    for file in "${files[@]}"; do
        if [ ! -f "$file" ]; then
            err "file not found: $file"
            violations=$((violations + 1))
            continue
        fi
        base="$(basename "$file")"
        while IFS= read -r entry; do
            [ -n "$entry" ] || continue
            lineno="${entry%%:*}"
            ref="${entry#*:}"
            [ "$ref" = "$channel" ] && continue
            if is_allowlisted "$base"; then
                warn "$base:$lineno uses dtolnay/rust-toolchain@$ref with targets: (expected @$channel) — allowlisted, tracked follow-up"
                allowlisted=$((allowlisted + 1))
            else
                err "$base:$lineno uses dtolnay/rust-toolchain@$ref with targets: — must pin to @$channel (rust-toolchain.toml). Floating refs drop cross-target rust-std at build time (issue #935, E0463)."
                violations=$((violations + 1))
            fi
        done < <(targets_bearing_refs "$file")
    done

    if [ "$allowlisted" -gt 0 ]; then
        echo "check-toolchain-refs: $allowlisted allowlisted (tracked) drift(s) reported above"
    fi

    if [ "$violations" -gt 0 ]; then
        err "$violations toolchain-ref drift violation(s) found. Pin the ref(s) to @$channel to keep cross-target rust-std in the resolved toolchain."
        return 1
    fi

    echo "check-toolchain-refs: OK — all targets-bearing toolchain refs pinned to @$channel"
    return 0
}

main "$@"
