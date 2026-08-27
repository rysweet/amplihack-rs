#!/usr/bin/env bash
# workflow_sync_package_version.sh — mirror the Cargo workspace version into the
# root package.json.
#
# Extracted verbatim from `step-14c-sync-package-json` in
# amplifier-bundle/recipes/workflow-publish.yaml. That brick was at its 400-line
# budget (scripts/check-brick-budget.sh), and the rule's remedy for a full brick
# is extraction, never compression — this is how the room was bought for the
# #1361 change in step-15 that stops a planning-only diff from emitting
# `Closes #N`. The logic below is unchanged; it is now directly testable from a
# shell rather than only through the recipe.
#
# Contract (issue #925): read the version through a SCOPED [workspace.package]
# parse or `cargo metadata --offline` — never an unscoped grep of Cargo.toml —
# and edit package.json with a real JSON tool (jq, or a python3 json.load
# fallback), atomically, never with sed. Every "cannot determine" path is a
# clean skip: this step mirrors a version, and must never be the thing that
# fails a publish.
#
# Usage: workflow_sync_package_version.sh [DIR]   (default: current directory)

set -euo pipefail
cd "${1:-.}"

# Issue #925: nothing syncs root package.json "version" after the workspace
# bump, so `package_json_version_matches_root_workspace_version` fails in CI.
# Sync package.json.version via a ROBUST scoped read + ROBUST JSON edit (jq,
# python3 fallback); skip gracefully when package.json is absent (non-JS workspace).
if [ ! -f package.json ]; then
  echo "step-14c: no package.json present; skipping npm version sync (non-JS workspace)"
  exit 0
fi
if [ ! -f Cargo.toml ]; then
  echo "step-14c: no Cargo.toml present; cannot determine workspace version; skipping"
  exit 0
fi
# Read [workspace.package].version via a SCOPED offline parse (NOT an unscoped
# grep that could match an unrelated table); fall back to `cargo metadata --offline`.
WS_VERSION="$(awk -F'"' '
  /^\[workspace\.package\]/ { in_ws = 1; next }
  /^\[/ { in_ws = 0 }
  in_ws && /^[[:space:]]*version[[:space:]]*=/ { print $2; exit }
' Cargo.toml)"
# Fallback: resolve the workspace ROOT package by its resolved id (.resolve.root),
# NOT `select(.source == null)` which matches ALL local members nondeterministically.
if [ -z "${WS_VERSION:-}" ] && command -v cargo >/dev/null 2>&1 && command -v jq >/dev/null 2>&1; then
  WS_VERSION="$(cargo metadata --no-deps --format-version=1 --offline 2>/dev/null \
    | jq -r '.resolve.root as $r | (.packages[] | select(.id == $r) | .version) // empty' 2>/dev/null || true)"
fi
if [ -z "${WS_VERSION:-}" ]; then
  # Non-standard layout or unreadable version: skip cleanly (mirror-only step; never a hard failure).
  echo "step-14c: could not read [workspace.package].version; skipping package.json sync"
  exit 0
fi
echo "step-14c: syncing package.json version to workspace version ${WS_VERSION}"
if command -v jq >/dev/null 2>&1; then
  # Same-directory temp file so `mv` is an atomic rename; preserve permissions.
  _tmp="$(mktemp ./package.json.XXXXXX)"
  trap 'rm -f "$_tmp"' EXIT
  chmod --reference=package.json "$_tmp" 2>/dev/null || true
  jq --arg v "$WS_VERSION" '.version = $v' package.json > "$_tmp"
  mv "$_tmp" package.json
  trap - EXIT
else
  # Same atomic contract as the jq path: same-directory temp + os.replace, mode preserved.
  WS_VERSION="$WS_VERSION" python3 -c 'import json, os, tempfile; p = "package.json"; d = json.load(open(p, encoding="utf-8")); d["version"] = os.environ["WS_VERSION"]; _dir = os.path.dirname(os.path.abspath(p)); _fd, _tmp = tempfile.mkstemp(dir=_dir, prefix="package.json."); os.write(_fd, (json.dumps(d, indent=2) + "\n").encode("utf-8")); os.close(_fd); os.chmod(_tmp, os.stat(p).st_mode & 0o7777); os.replace(_tmp, p)'
fi
