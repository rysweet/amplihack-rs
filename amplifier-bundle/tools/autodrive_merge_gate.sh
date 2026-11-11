#!/bin/bash -p
# Trusted launch adapter; readiness, revalidation and single-attempt merging live in Rust.
set -uo pipefail
builtin unset -f gh git jq amplihack
STATE_DIR=""; REPO="."; PR=""; DRY_RUN=false
while [ $# -gt 0 ]; do
  case "$1" in
    --pr) PR="${2:-}"; shift 2 ;;
    --repo) REPO="${2:-}"; shift 2 ;;
    --state-dir) STATE_DIR="${2:-}"; shift 2 ;;
    --round-record|--qa-evidence) shift 2 ;; # Compatibility: Rust uses launch-registered references.
    --dry-run) DRY_RUN=true; shift ;;
    *) echo "ERROR: unknown gate argument" >&2; exit 2 ;;
  esac
done
case "$PR" in ''|0|*[!0-9]*) echo "ERROR: --pr must be positive" >&2; exit 2 ;; esac
[ -n "$STATE_DIR" ] || { echo "ERROR: --state-dir required" >&2; exit 2; }
cd "$REPO" || exit 2
ARGS=(pr merge gate --state-dir "$STATE_DIR")
[ "$DRY_RUN" = false ] || ARGS+=(--dry-run)
# Keep this shell as the parent: both Rust and gh's guard inspect its actual fd/255.
"${AMPLIHACK_BIN:-amplihack}" "${ARGS[@]}"
