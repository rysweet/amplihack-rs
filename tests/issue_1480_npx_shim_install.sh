#!/usr/bin/env bash
# Outside-in check for issue #1480: `npx … amplihack install` must not abort on
# the shim npx itself puts first on PATH.
#
# While `npx --package=<amplihack-rs> -- amplihack install` runs, npm prepends
# `<cache>/_npx/<hash>/node_modules/.bin` to PATH, and that directory's
# `amplihack` is a symlink to this package's own `npm/bin/amplihack.js`.
# Before the fix, install classified the symlink as an unknown executable that
# shadows `~/.local/bin/amplihack` and failed with
# "failed to neutralize stale Python/uvx amplihack PATH wrappers".
#
# This drives the REAL `amplihack` binary against that exact PATH layout in a
# throwaway HOME. It checks three things:
#   1. The shim alone: install succeeds, leaves the shim in place, prints the
#      npx notice, and does not tell the user to reorder PATH.
#   2. An unrelated script at the same `_npx` path: install still refuses it.
#   3. A persistent executable behind the shim: install still refuses it, and
#      the PATH advisory names that executable rather than the shim.
#   4. The sh shim `pnpm dlx` writes for our wrapper: transient too (#1496).
#   5. The persistent `npm install -g` launcher for our wrapper: install
#      completes and the advisory says the npm launcher wins (#1496).
#   6. Case 5 in the PATH shape `amplihack update` uses to re-run install:
#      the ambiguity advisory explains the launcher too, never "stale".
#
# Usage: AMPLIHACK_BIN=/path/to/amplihack bash tests/issue_1480_npx_shim_install.sh
# (defaults to the `amplihack` on PATH). CI runs it in the Install Smoke Test
# job, right after `cargo install --path bins/amplihack`.

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
BIN="${AMPLIHACK_BIN:-$(command -v amplihack || true)}"
if [[ -z "$BIN" || ! -x "$BIN" ]]; then
  echo "FAIL: no amplihack binary; set AMPLIHACK_BIN" >&2
  exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

STUBS="$TMP/stubs"
mkdir -p "$STUBS"
for stub in amplihack-hooks recipe-runner-rs; do
  printf '#!/bin/sh\nexit 0\n' > "$STUBS/$stub"
  chmod +x "$STUBS/$stub"
done

failures=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

# make_npx_layout HOME WRAPPER_SOURCE -> prints the shim's .bin directory.
make_npx_layout() {
  local home="$1" source="$2"
  local modules="$home/.npm/_npx/20a160db8db9e1ce/node_modules"
  mkdir -p "$modules/@rysweet/amplihack-rs/npm/bin" "$modules/.bin" "$home/.local/bin"
  cp "$source" "$modules/@rysweet/amplihack-rs/npm/bin/amplihack.js"
  chmod +x "$modules/@rysweet/amplihack-rs/npm/bin/amplihack.js"
  ln -s ../@rysweet/amplihack-rs/npm/bin/amplihack.js "$modules/.bin/amplihack"
  printf '%s\n' "$modules/.bin"
}

# make_pnpm_dlx_layout HOME WRAPPER_SOURCE -> prints the shim's .bin directory.
# What `pnpm dlx` does (issue #1496): an sh shim, not a symlink.
make_pnpm_dlx_layout() {
  local home="$1" source="$2"
  local run="$home/.cache/pnpm/dlx/8528939ac0074566c55bd016/1a0df37544d-1dc2/node_modules"
  local pkg_rel=".pnpm/@rysweet+amplihack-rs@0.18.0/node_modules/@rysweet/amplihack-rs"
  mkdir -p "$run/$pkg_rel/npm/bin" "$run/.bin" "$home/.local/bin"
  cp "$source" "$run/$pkg_rel/npm/bin/amplihack.js"
  cat > "$run/.bin/amplihack" <<EOF
#!/bin/sh
basedir=\$(dirname "\$(echo "\$0" | sed -e 's,\\\\,/,g')")

if [ -x "\$basedir/node" ]; then
  exec "\$basedir/node"  "\$basedir/../$pkg_rel/npm/bin/amplihack.js" "\$@"
else
  exec node  "\$basedir/../$pkg_rel/npm/bin/amplihack.js" "\$@"
fi
EOF
  chmod +x "$run/.bin/amplihack"
  printf '%s\n' "$run/.bin"
}

# make_npm_global_layout PREFIX WRAPPER_SOURCE -> prints the prefix's bin dir.
# What `npm install -g @rysweet/amplihack-rs` does: a persistent symlink.
make_npm_global_layout() {
  local prefix="$1" source="$2"
  mkdir -p "$prefix/lib/node_modules/@rysweet/amplihack-rs/npm/bin" "$prefix/bin"
  cp "$source" "$prefix/lib/node_modules/@rysweet/amplihack-rs/npm/bin/amplihack.js"
  ln -s ../lib/node_modules/@rysweet/amplihack-rs/npm/bin/amplihack.js "$prefix/bin/amplihack"
  printf '%s\n' "$prefix/bin"
}

# run_install HOME PATH OUTFILE [ENV=VALUE...] -> install's exit status.
run_install() {
  local home="$1" path="$2" out="$3" status=0
  shift 3
  mkdir -p "$home/work"
  (
    cd "$home/work"
    env -i \
      HOME="$home" \
      PATH="$path" \
      TMPDIR="$TMP" \
      AMPLIHACK_SKIP_MMDC=1 \
      AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH="$STUBS/amplihack-hooks" \
      RECIPE_RUNNER_RS_PATH="$STUBS/recipe-runner-rs" \
      "$@" \
      "$BIN" install --local "$ROOT"
  ) > "$out" 2>&1 || status=$?
  return "$status"
}

SYSTEM_PATH="/usr/bin:/bin"

# --- 1. the shim for our own wrapper: install completes ---------------------
home="$TMP/case1"
npx_bin="$(make_npx_layout "$home" "$ROOT/npm/bin/amplihack.js")"
out="$TMP/case1.log"
status=0
run_install "$home" "$npx_bin:$home/.local/bin:$SYSTEM_PATH" "$out" || status=$?
if [[ "$status" -eq 0 ]]; then
  pass "install succeeds with the npx shim first on PATH"
else
  fail "install exited $status with the npx shim first on PATH"
  cat "$out" >&2
fi
if grep -qF "failed to neutralize" "$out"; then
  fail "install still reports the npx shim as a PATH conflict"
else
  pass "no neutralizer failure"
fi
if grep -qF "Leaving transient npx/dlx shim $npx_bin/amplihack in place" "$out"; then
  pass "install prints the transient npx shim notice"
else
  fail "missing transient npx shim notice"
fi
if grep -qF "Reorder PATH" "$out"; then
  fail "PATH advisory tells the user to reorder PATH around the npx shim"
else
  pass "no contradictory reorder-PATH advice"
fi
if [[ -L "$npx_bin/amplihack" ]]; then
  pass "npx shim left in place"
else
  fail "npx shim was moved or deleted"
fi

# --- 2. an unrelated script at the npx path: still refused ------------------
home="$TMP/case2"
printf '#!/usr/bin/env node\nconsole.log("other");\n' > "$TMP/other.js"
npx_bin="$(make_npx_layout "$home" "$TMP/other.js")"
out="$TMP/case2.log"
status=0
run_install "$home" "$npx_bin:$home/.local/bin:$SYSTEM_PATH" "$out" || status=$?
if [[ "$status" -ne 0 ]] \
  && grep -qF "unknown executable $npx_bin/amplihack shadows" "$out"; then
  pass "an unrelated script under _npx still blocks install"
else
  fail "an unrelated script under _npx was accepted (exit $status)"
  cat "$out" >&2
fi

# --- 3. a persistent executable behind the shim: still refused --------------
home="$TMP/case3"
npx_bin="$(make_npx_layout "$home" "$ROOT/npm/bin/amplihack.js")"
persistent_dir="$TMP/case3-system-bin"
mkdir -p "$persistent_dir"
printf '#!/bin/sh\necho persistent\n' > "$persistent_dir/amplihack"
chmod +x "$persistent_dir/amplihack"
out="$TMP/case3.log"
status=0
run_install "$home" "$npx_bin:$persistent_dir:$home/.local/bin:$SYSTEM_PATH" "$out" \
  || status=$?
if [[ "$status" -ne 0 ]] \
  && grep -qF "unknown executable $persistent_dir/amplihack shadows" "$out"; then
  pass "a persistent executable behind the shim still blocks install"
else
  fail "a persistent executable behind the shim was accepted (exit $status)"
  cat "$out" >&2
fi
if grep -qF "\`amplihack\` at $persistent_dir/amplihack shadows" "$out"; then
  pass "PATH advisory names the persistent shadow, not the shim"
else
  fail "PATH advisory did not name the persistent shadow"
  cat "$out" >&2
fi

# --- 4. pnpm dlx sh shim for our wrapper (issue #1496): install completes ---
home="$TMP/case4"
pnpm_bin="$(make_pnpm_dlx_layout "$home" "$ROOT/npm/bin/amplihack.js")"
out="$TMP/case4.log"
status=0
run_install "$home" "$pnpm_bin:$home/.local/bin:$SYSTEM_PATH" "$out" || status=$?
if [[ "$status" -eq 0 ]] && grep -qF "Leaving transient npx/dlx shim $pnpm_bin/amplihack in place" "$out"; then
  pass "install succeeds with the pnpm dlx sh shim first on PATH"
else
  fail "install exited $status with the pnpm dlx shim first on PATH"
  cat "$out" >&2
fi
if [[ -f "$pnpm_bin/amplihack" ]] && ! grep -qF "Reorder PATH" "$out"; then
  pass "pnpm dlx shim left in place, no reorder-PATH advice"
else
  fail "pnpm dlx shim removed or flagged"
fi

# --- 5. npm -g launcher ahead of ~/.local/bin (issue #1496): completes, explains -
home="$TMP/case5"
mkdir -p "$home/.local/bin"
global_bin="$(make_npm_global_layout "$TMP/case5-prefix" "$ROOT/npm/bin/amplihack.js")"
out="$TMP/case5.log"
status=0
run_install "$home" "$global_bin:$home/.local/bin:$SYSTEM_PATH" "$out" || status=$?
if [[ "$status" -eq 0 ]] && ! grep -qF "failed to neutralize" "$out"; then
  pass "install succeeds with a persistent npm -g launcher first on PATH"
else
  fail "install exited $status with an npm -g launcher first on PATH"
  cat "$out" >&2
fi
if grep -qF "is the npm launcher for this package" "$out" \
  && grep -qF "npm uninstall -g @rysweet/amplihack-rs" "$out"; then
  pass "advisory says the npm launcher wins and how to change that"
else
  fail "advisory did not explain the persistent npm launcher"
  cat "$out" >&2
fi
if [[ -L "$global_bin/amplihack" ]]; then
  pass "npm -g launcher left in place"
else
  fail "npm -g launcher was moved or deleted"
fi

# --- 6. same launcher, but in the shape `amplihack update` re-runs install:
#        ~/.local/bin moved first, original PATH in AMPLIHACK_REPAIR_ORIGINAL_PATH.
home="$TMP/case6"
mkdir -p "$home/.local/bin"
global_bin="$(make_npm_global_layout "$TMP/case6-prefix" "$ROOT/npm/bin/amplihack.js")"
out="$TMP/case6.log"
status=0
run_install "$home" "$home/.local/bin:$global_bin:$SYSTEM_PATH" "$out" \
  AMPLIHACK_REPAIR_ORIGINAL_PATH="$global_bin:$home/.local/bin:$SYSTEM_PATH" || status=$?
if [[ "$status" -eq 0 ]] && grep -qF "npm uninstall -g @rysweet/amplihack-rs" "$out" \
  && ! grep -qF "Remove stale candidates" "$out"; then
  pass "update-shaped install explains the npm launcher, never calls it stale"
else
  fail "update-shaped install mis-describes the npm launcher (exit $status)"
  cat "$out" >&2
fi

if [[ "$failures" -ne 0 ]]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "All issue #1480 npx shim install checks passed."
