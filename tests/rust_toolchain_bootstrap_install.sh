#!/usr/bin/env bash
# Outside-in check: `amplihack install` on a machine with no Rust toolchain.
#
# A fresh VM (e.g. Ubuntu via the README's `npx ... -- amplihack install`) has
# no cargo and usually no C compiler, so the recipe-runner phase used to fail
# with "cargo is required to install recipe-runner-rs". Install now bootstraps
# rustup (user-local) and, when possible without a password, build-essential.
#
# This drives the REAL `amplihack` binary in a throwaway HOME with a sandboxed
# PATH that hides the host's cargo/cc. The network-facing and privileged tools
# are stubs: `curl` serves a fake rustup-init.sh that creates a fake cargo,
# whose `install` drops a recipe-runner-rs into $HOME/.cargo/bin; `sudo`/
# `apt-get` stand in for the build-essential install. Cases:
#   1. no cargo, C compiler present: rustup is bootstrapped, install succeeds,
#      and rustup-init is run with --no-modify-path (so real rustup leaves
#      shell profiles alone).
#   2. no cargo, no C compiler, passwordless sudo + apt-get: build-essential is
#      installed non-interactively (after retrying an `apt-get update` that
#      first fails on a held lists lock), install succeeds.
#   3. no C compiler and sudo that needs a password: install fails with the
#      exact apt-get command and never runs apt-get.
#   4. AMPLIHACK_NO_RUST_BOOTSTRAP=1: install fails fast, nothing downloaded.
#   5. a non-default CARGO_HOME that is not on PATH: the installed
#      recipe-runner-rs is still found.
#   6. `apt-get update` failing for a non-lock reason (broken repo): not
#      retried; the build-essential install still goes ahead.
#
# Usage: AMPLIHACK_BIN=/path/to/amplihack bash tests/rust_toolchain_bootstrap_install.sh
# (defaults to the `amplihack` on PATH). CI runs it in the Install Smoke Test job.

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
BIN="${AMPLIHACK_BIN:-$(command -v amplihack || true)}"
if [[ -z "$BIN" || ! -x "$BIN" ]]; then
  echo "FAIL: no amplihack binary; set AMPLIHACK_BIN" >&2
  exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

failures=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

# Sandbox PATH: only general-purpose tools, never cargo/rustc/cc/gcc/clang/curl.
SANDBOX="$TMP/sandbox-bin"
mkdir -p "$SANDBOX"
for tool in sh bash env cat cp mv rm ln mkdir chmod touch printf test true false \
  uname id grep sed awk head tail tr dirname basename readlink mktemp git; do
  if real="$(command -v "$tool" 2>/dev/null)" && [[ -x "$real" ]]; then
    ln -sf "$real" "$SANDBOX/$tool"
  fi
done

STUBS="$TMP/stubs"
mkdir -p "$STUBS"
printf '#!/bin/sh\nexit 0\n' > "$STUBS/amplihack-hooks"
chmod +x "$STUBS/amplihack-hooks"

# Fake rustup-init.sh: installs a fake cargo whose `install` places
# recipe-runner-rs next to itself; records its argv for assertions.
cat > "$TMP/rustup-init.sh" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" > "$HOME/rustup-init.args"
bin="${CARGO_HOME:-$HOME/.cargo}/bin"
mkdir -p "$bin"
cat > "$bin/cargo" <<'CARGO'
#!/bin/sh
printf '%s\n' "$*" > "$HOME/cargo.args"
[ "$1" = install ] || exit 1
dir="$(dirname "$0")"
printf '#!/bin/sh\nexit 0\n' > "$dir/recipe-runner-rs"
chmod +x "$dir/recipe-runner-rs"
CARGO
chmod +x "$bin/cargo"
EOF

# make_tools DIR [curl] [cc] [apt] [sudo-ok|sudo-password]
make_tools() {
  local dir="$1"
  shift
  mkdir -p "$dir"
  for tool in "$@"; do
    case "$tool" in
      curl)
        cat > "$dir/curl" <<EOF
#!/bin/sh
out=""
while [ \$# -gt 0 ]; do
  case "\$1" in -o) out="\$2"; shift ;; esac
  shift
done
[ -n "\$out" ] || exit 2
touch "\$HOME/curl.called"
cp "$TMP/rustup-init.sh" "\$out"
EOF
        ;;
      cc) printf '#!/bin/sh\nexit 0\n' > "$dir/cc" ;;
      apt)
        cat > "$dir/apt-get" <<EOF
#!/bin/sh
printf '%s DEBIAN_FRONTEND=%s LC_ALL=%s\n' "\$*" "\${DEBIAN_FRONTEND:-}" "\${LC_ALL:-}" >> "\$HOME/apt.log"
# First \`update\` fails like apt-daily holding /var/lib/apt/lists/lock.
case " \$* " in
  *" update "*)
    if [ ! -e "\$HOME/apt.update.busy" ]; then
      touch "\$HOME/apt.update.busy"
      echo "E: Could not get lock /var/lib/apt/lists/lock" >&2
      exit 100
    fi ;;
esac
case " \$* " in
  *" install "*) printf '#!/bin/sh\nexit 0\n' > "$dir/cc"; chmod +x "$dir/cc" ;;
esac
EOF
        ;;
      apt-broken)
        # `update` always fails like a PPA with a missing key; install works.
        cat > "$dir/apt-get" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "\$HOME/apt.log"
case " \$* " in
  *" update "*) echo "E: The repository is not signed. NO_PUBKEY 0123" >&2; exit 100 ;;
  *" install "*) printf '#!/bin/sh\nexit 0\n' > "$dir/cc"; chmod +x "$dir/cc" ;;
esac
EOF
        ;;
      sudo-ok)
        cat > "$dir/sudo" <<'EOF'
#!/bin/sh
[ "$1" = -n ] && shift
[ "$1" = true ] && exit 0
exec "$@"
EOF
        ;;
      sudo-password)
        printf '#!/bin/sh\necho "sudo: a password is required" >&2\nexit 1\n' > "$dir/sudo"
        ;;
    esac
  done
  chmod +x "$dir"/* 2>/dev/null || true
}

# run_install HOME TOOLS_DIR OUTFILE [ENV...] -> install's exit status.
run_install() {
  local home="$1" tools="$2" out="$3" status=0
  shift 3
  mkdir -p "$home/work" "$home/.local/bin"
  (
    cd "$home/work"
    env -i \
      HOME="$home" \
      PATH="$tools:$home/.local/bin:$SANDBOX" \
      TMPDIR="$TMP" \
      AMPLIHACK_SKIP_MMDC=1 \
      AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH="$STUBS/amplihack-hooks" \
      "$@" \
      "$BIN" install --local "$ROOT"
  ) > "$out" 2>&1 || status=$?
  return "$status"
}

# --- 1. no cargo, C compiler present ----------------------------------------
home="$TMP/case1"
make_tools "$TMP/case1-tools" curl cc
out="$TMP/case1.log"
status=0
run_install "$home" "$TMP/case1-tools" "$out" || status=$?
if [[ "$status" -eq 0 ]]; then
  pass "install succeeds with no cargo on PATH"
else
  fail "install exited $status with no cargo on PATH"
  cat "$out" >&2
fi
if [[ -x "$home/.cargo/bin/recipe-runner-rs" ]]; then
  pass "recipe-runner-rs installed into ~/.cargo/bin by the bootstrapped cargo"
else
  fail "recipe-runner-rs missing from ~/.cargo/bin"
fi
if grep -qx -- "-y --no-modify-path --profile minimal" "$home/rustup-init.args" 2>/dev/null; then
  pass "rustup-init ran with -y --no-modify-path --profile minimal"
else
  fail "rustup-init args wrong: $(cat "$home/rustup-init.args" 2>/dev/null || echo '<not run>')"
fi
if grep -q "^install --git https://github.com/rysweet/amplihack-recipe-runner" "$home/cargo.args" 2>/dev/null; then
  pass "bootstrapped cargo installed recipe-runner-rs from git"
else
  fail "cargo was not asked to install recipe-runner-rs"
fi
if grep -qF "installing a minimal Rust toolchain with rustup" "$out"; then
  pass "install tells the user it is installing Rust"
else
  fail "missing rustup bootstrap notice"
fi

# --- 2. no cargo, no C compiler, passwordless sudo + apt-get ----------------
home="$TMP/case2"
make_tools "$TMP/case2-tools" curl apt sudo-ok
out="$TMP/case2.log"
status=0
run_install "$home" "$TMP/case2-tools" "$out" || status=$?
if [[ "$status" -eq 0 ]]; then
  pass "install succeeds after installing build-essential"
else
  fail "install exited $status with no C compiler"
  cat "$out" >&2
fi
if grep -q "^-o DPkg::Lock::Timeout=300 install -y -qq build-essential DEBIAN_FRONTEND=noninteractive LC_ALL=C$" "$home/apt.log" 2>/dev/null; then
  pass "build-essential installed non-interactively, in the C locale"
else
  fail "apt-get install build-essential not run as expected: $(cat "$home/apt.log" 2>/dev/null || echo '<not run>')"
fi
if [[ "$(grep -c " update -qq " "$home/apt.log" 2>/dev/null)" -eq 2 ]]; then
  pass "apt-get update retried while apt lists were locked"
else
  fail "apt-get update was not retried after a lock failure"
fi

# --- 3. no C compiler, sudo needs a password ----------------------------------
# Only meaningful when not root: root runs apt-get directly and needs no sudo.
if [[ "$(id -u)" -ne 0 ]]; then
  home="$TMP/case3"
  make_tools "$TMP/case3-tools" curl apt sudo-password
  out="$TMP/case3.log"
  status=0
  run_install "$home" "$TMP/case3-tools" "$out" || status=$?
  if [[ "$status" -ne 0 ]] && grep -qF "sudo apt-get install -y build-essential" "$out"; then
    pass "password-protected sudo fails with the exact command to run"
  else
    fail "expected failure naming the apt-get command (exit $status)"
    cat "$out" >&2
  fi
  if [[ ! -e "$home/apt.log" ]]; then
    pass "apt-get never ran without passwordless sudo"
  else
    fail "apt-get ran even though sudo needed a password"
  fi
else
  echo "SKIP: case 3 (running as root, sudo is not used)"
fi

# --- 4. AMPLIHACK_NO_RUST_BOOTSTRAP=1 ----------------------------------------
home="$TMP/case4"
make_tools "$TMP/case4-tools" curl cc
out="$TMP/case4.log"
status=0
run_install "$home" "$TMP/case4-tools" "$out" AMPLIHACK_NO_RUST_BOOTSTRAP=1 || status=$?
if [[ "$status" -ne 0 ]] && grep -qF "cargo is required to install recipe-runner-rs" "$out"; then
  pass "opt-out fails with the manual install hint"
else
  fail "opt-out did not fail as expected (exit $status)"
  cat "$out" >&2
fi
if [[ ! -e "$home/curl.called" && ! -e "$home/.cargo" ]]; then
  pass "opt-out downloads and installs nothing"
else
  fail "opt-out still bootstrapped Rust"
fi

# --- 5. non-default CARGO_HOME, not on PATH ---------------------------------
home="$TMP/case5"
make_tools "$TMP/case5-tools" curl cc
out="$TMP/case5.log"
status=0
run_install "$home" "$TMP/case5-tools" "$out" CARGO_HOME="$home/custom-cargo" || status=$?
if [[ "$status" -eq 0 && -x "$home/custom-cargo/bin/recipe-runner-rs" ]]; then
  pass "recipe-runner-rs in a custom \$CARGO_HOME/bin is found after install"
else
  fail "custom CARGO_HOME install failed (exit $status)"
  cat "$out" >&2
fi

# --- 6. apt-get update fails for a non-lock reason: no waiting --------------
home="$TMP/case6"
make_tools "$TMP/case6-tools" curl apt-broken sudo-ok
out="$TMP/case6.log"
status=0
run_install "$home" "$TMP/case6-tools" "$out" || status=$?
if [[ "$status" -eq 0 && "$(grep -c "update -qq" "$home/apt.log" 2>/dev/null)" -eq 1 ]]; then
  pass "a permanent apt-get update error is not retried; install proceeds"
else
  fail "permanent update error handling wrong (exit $status): $(cat "$home/apt.log" 2>/dev/null)"
  cat "$out" >&2
fi

if [[ "$failures" -ne 0 ]]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "All Rust toolchain bootstrap install checks passed."
