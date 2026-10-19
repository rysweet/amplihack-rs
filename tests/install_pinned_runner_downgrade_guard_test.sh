#!/usr/bin/env bash
# Tests for the recipe-runner installer downgrade guard (issue #1593).
#
# Covers three things, all offline:
#   1. scripts/lib/runner-ancestry.sh: runner_receipt_revision and
#      runner_relation, called directly.
#   2. scripts/install-pinned-recipe-runner.sh (AC6): a copy of the script in a
#      temporary repository root with a fixture pin, a stub Cargo receipt and
#      binary under $RUNNER_TEMP/recipe-runner, a stub cargo that logs its
#      arguments, and RECIPE_RUNNER_ANCESTRY_REPO pointing at a fixture.
#   3. claude-plugin/bin/install-runtime: the same skip and force cases.
#
# Fixture runner history:   A --- B --- C   (main)
#                            \
#                             D             (side)
# The pin is always B. Installed C is Newer, A is Older, D is Diverged.
#
# Run: bash tests/install_pinned_runner_downgrade_guard_test.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LIB="$REPO_ROOT/scripts/lib/runner-ancestry.sh"
INSTALLER="$REPO_ROOT/scripts/install-pinned-recipe-runner.sh"
INSTALL_RUNTIME="$REPO_ROOT/claude-plugin/bin/install-runtime"
RUNNER_URL="https://github.com/rysweet/amplihack-recipe-runner"

pass=0
fail=0
TMPROOT=""

cleanup() { [ -n "$TMPROOT" ] && rm -rf "$TMPROOT"; }
trap cleanup EXIT

record_pass() {
    echo "PASS: $1"
    pass=$((pass + 1))
}

record_fail() {
    echo "FAIL: $1"
    fail=$((fail + 1))
}

TMPROOT="$(mktemp -d)"

fgit() {
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
        GIT_AUTHOR_NAME=Fixture GIT_AUTHOR_EMAIL=fixture@example.invalid \
        GIT_COMMITTER_NAME=Fixture GIT_COMMITTER_EMAIL=fixture@example.invalid \
        git -c commit.gpgsign=false -c init.defaultBranch=main "$@"
}

# ---------------------------------------------------------------------------
# Runner fixture
# ---------------------------------------------------------------------------
ANCESTRY="$TMPROOT/runner"
mkdir -p "$ANCESTRY"
fgit -C "$ANCESTRY" init -q
fgit -C "$ANCESTRY" config uploadpack.allowFilter true
fgit -C "$ANCESTRY" config uploadpack.allowAnySHA1InWant true
fgit -C "$ANCESTRY" commit -q --allow-empty -m A
SHA_A="$(fgit -C "$ANCESTRY" rev-parse HEAD)"
fgit -C "$ANCESTRY" commit -q --allow-empty -m B
SHA_B="$(fgit -C "$ANCESTRY" rev-parse HEAD)"
fgit -C "$ANCESTRY" commit -q --allow-empty -m C
SHA_C="$(fgit -C "$ANCESTRY" rev-parse HEAD)"
fgit -C "$ANCESTRY" checkout -q -b side "$SHA_A"
fgit -C "$ANCESTRY" commit -q --allow-empty -m D
SHA_D="$(fgit -C "$ANCESTRY" rev-parse HEAD)"
fgit -C "$ANCESTRY" checkout -q main
SHA_MISSING="0123456789abcdef0123456789abcdef01234567"
PIN="$SHA_B"

short() { printf '%s' "${1:0:12}"; }

# receipt <file> <sha> [url]: a Cargo .crates2.json naming recipe-runner-rs.
receipt() {
    local url="${3:-$RUNNER_URL}"
    mkdir -p "$(dirname "$1")"
    printf '{"installs":{"recipe-runner-rs 0.5.0 (git+%s#%s)":{"bins":["recipe-runner-rs"]}}}\n' \
        "$url" "$2" >"$1"
}

# ===========================================================================
# 1. scripts/lib/runner-ancestry.sh
# ===========================================================================
if [ ! -f "$LIB" ]; then
    record_fail "scripts/lib/runner-ancestry.sh exists"
fi

# lib_call <function> <args...>: run one library function under POSIX sh.
LIB_OUT="$TMPROOT/lib-out"
lib_call() {
    sh -c '. "$0" && "$@"' "$LIB" "$@" >"$LIB_OUT" 2>/dev/null
    LIB_STATUS=$?
}

lib_expect() {
    # lib_expect <expected stdout> <label>
    local got
    got="$(cat "$LIB_OUT")"
    if [ "$got" = "$1" ]; then
        record_pass "$2"
    else
        record_fail "$2: expected '$1', got '$got'"
    fi
}

R="$TMPROOT/lib-receipt.json"

receipt "$R" "$SHA_C"
lib_call runner_receipt_revision "$R" "$RUNNER_URL"
lib_expect "$SHA_C" "receipt: exact key yields the SHA"

printf '{"installs":{"recipe-runner-rs 0.5.0 (git+%s?rev=%s#%s)":{"bins":["recipe-runner-rs"]}}}\n' \
    "$RUNNER_URL" "$SHA_C" "$SHA_C" >"$R"
lib_call runner_receipt_revision "$R" "$RUNNER_URL"
lib_expect "$SHA_C" "receipt: URL with a query string yields the SHA"

receipt "$R" "$SHA_C" "$RUNNER_URL-evil"
lib_call runner_receipt_revision "$R" "$RUNNER_URL"
lib_expect "" "receipt: lookalike '-evil' URL is ignored"

receipt "$R" "$SHA_C" "https://github.com/someone/amplihack-recipe-runner"
lib_call runner_receipt_revision "$R" "$RUNNER_URL"
lib_expect "" "receipt: another repository is ignored"

receipt "$R" "${SHA_C:0:39}"
lib_call runner_receipt_revision "$R" "$RUNNER_URL"
lib_expect "" "receipt: 39-character SHA is ignored"

receipt "$R" "$(printf '%s' "$SHA_C" | tr '[:lower:]' '[:upper:]')"
lib_call runner_receipt_revision "$R" "$RUNNER_URL"
lib_expect "" "receipt: uppercase SHA is ignored"

printf '{"installs":{"recipe-runner-rs 0.5.0 (git+%s#%s)":{"bins":["recipe-runner-rs"]},"recipe-runner-rs 0.4.0 (git+%s#%s)":{"bins":["recipe-runner-rs"]}}}\n' \
    "$RUNNER_URL" "$SHA_C" "$RUNNER_URL" "$SHA_A" >"$R"
lib_call runner_receipt_revision "$R" "$RUNNER_URL"
lib_expect "" "receipt: two distinct runner entries are ambiguous"

lib_call runner_receipt_revision "$TMPROOT/no-such-receipt.json" "$RUNNER_URL"
lib_expect "" "receipt: missing file yields nothing"
if [ "$LIB_STATUS" -eq 0 ]; then
    record_pass "receipt: missing file still exits 0"
else
    record_fail "receipt: missing file exited $LIB_STATUS"
fi

# Over 1 MiB: rejected even though the key is valid.
receipt "$R" "$SHA_C"
awk 'BEGIN { for (i = 0; i < 1100; i++) { s = ""; for (j = 0; j < 1024; j++) s = s " "; print s } }' >>"$R"
lib_call runner_receipt_revision "$R" "$RUNNER_URL"
lib_expect "" "receipt: file over 1 MiB is rejected"

lib_call runner_relation "$ANCESTRY" "$SHA_B" "$SHA_B"
lib_expect "equal" "relation: same SHA is equal"
lib_call runner_relation "$ANCESTRY" "$SHA_C" "$SHA_B"
lib_expect "descendant" "relation: C is a descendant of B"
lib_call runner_relation "$ANCESTRY" "$SHA_A" "$SHA_B"
lib_expect "ancestor" "relation: A is an ancestor of B"
lib_call runner_relation "$ANCESTRY" "$SHA_D" "$SHA_B"
lib_expect "diverged" "relation: D has diverged from B"
lib_call runner_relation "$ANCESTRY" "$SHA_MISSING" "$SHA_B"
lib_expect "missing" "relation: unknown SHA is missing"
lib_call runner_relation "$ANCESTRY" "--upload-pack=x" "$SHA_B"
got="$(cat "$LIB_OUT")"
case "$got" in
    equal | descendant | ancestor | diverged)
        record_fail "relation: option-shaped SHA was classified as '$got'"
        ;;
    *)
        record_pass "relation: option-shaped SHA is not classified"
        ;;
esac

# ===========================================================================
# 2. scripts/install-pinned-recipe-runner.sh
# ===========================================================================
case_n=0
CASE=""
OUT=""
STATUS=0

# setup_case <installed sha | "-" for no receipt> [receipt url]
# Builds a fresh repository copy, stub cargo and stub installed runner.
setup_case() {
    case_n=$((case_n + 1))
    CASE="$TMPROOT/case$case_n"
    mkdir -p "$CASE/repo/scripts/lib" "$CASE/repo/claude-plugin" "$CASE/bin" \
        "$CASE/runner-temp/recipe-runner/bin" "$CASE/cargo-home"
    cp "$INSTALLER" "$CASE/repo/scripts/"
    [ -f "$LIB" ] && cp "$LIB" "$CASE/repo/scripts/lib/"
    printf '%s\n' "$PIN" >"$CASE/repo/claude-plugin/recipe-runner.rev"
    : >"$CASE/github-path"
    : >"$CASE/github-env"
    OUT="$CASE/out"

    # Stub installed runner: logs every invocation.
    cat >"$CASE/runner-temp/recipe-runner/bin/recipe-runner-rs" <<STUB
#!/bin/sh
printf '%s\n' "\$*" >> "$CASE/runner-log"
case "\$1" in
  --version) echo "recipe-runner 0.5.0" ;;
  --capabilities) echo '{"schema_version":1,"version":"0.5.0","capabilities":["codex_exec"]}' ;;
esac
STUB
    chmod +x "$CASE/runner-temp/recipe-runner/bin/recipe-runner-rs"
    if [ "$1" != "-" ]; then
        receipt "$CASE/runner-temp/recipe-runner/.crates2.json" "$1" "${2:-$RUNNER_URL}"
    fi

    # Stub cargo: logs its arguments, one per line, then leaves the runner in
    # place and rewrites the receipt for the requested revision.
    cat >"$CASE/bin/cargo" <<STUB
#!/bin/sh
for a in "\$@"; do printf '%s\n' "\$a"; done >> "$CASE/cargo-log"
echo "--" >> "$CASE/cargo-log"
rev=""
prev=""
for a in "\$@"; do
  [ "\$prev" = "--rev" ] && rev="\$a"
  prev="\$a"
done
printf '{"installs":{"recipe-runner-rs 0.5.0 (git+$RUNNER_URL#%s)":{"bins":["recipe-runner-rs"]}}}\n' "\$rev" > "$CASE/runner-temp/recipe-runner/.crates2.json"
exit 0
STUB
    chmod +x "$CASE/bin/cargo"
}

# run_installer [args...]: environment comes from the caller's exports.
run_installer() {
    PATH="$CASE/bin:$PATH" RUNNER_TEMP="$CASE/runner-temp" \
        CARGO_HOME="$CASE/cargo-home" \
        RECIPE_RUNNER_ANCESTRY_REPO="${ANCESTRY_OVERRIDE-$ANCESTRY}" \
        GITHUB_PATH="$CASE/github-path" GITHUB_ENV="$CASE/github-env" \
        bash "$CASE/repo/scripts/install-pinned-recipe-runner.sh" "$@" >"$OUT" 2>&1
    STATUS=$?
}

cargo_called() { [ -s "$CASE/cargo-log" ]; }

expect_status() {
    if [ "$STATUS" -eq "$1" ]; then
        record_pass "$2 (exit $1)"
    else
        record_fail "$2: expected exit $1, got $STATUS"
        sed 's/^/    | /' "$OUT"
    fi
}

expect_cargo() {
    # expect_cargo yes|no <label>
    if [ "$1" = yes ]; then
        if cargo_called; then record_pass "$2"; else
            record_fail "$2: cargo was not called"
            sed 's/^/    | /' "$OUT"
        fi
    else
        if cargo_called; then
            record_fail "$2: cargo was called"
            sed 's/^/    cargo| /' "$CASE/cargo-log"
        else record_pass "$2"; fi
    fi
}

expect_out() {
    if grep -qF -- "$1" "$OUT"; then record_pass "$2"; else
        record_fail "$2: output lacks '$1'"
        sed 's/^/    | /' "$OUT"
    fi
}

expect_out_ci() {
    if grep -qiF -- "$1" "$OUT"; then record_pass "$2"; else
        record_fail "$2: output lacks '$1' (any case)"
        sed 's/^/    | /' "$OUT"
    fi
}

expect_no_out() {
    if grep -qiF -- "$1" "$OUT"; then
        record_fail "$2: output contains '$1'"
    else record_pass "$2"; fi
}

# The pre-#1593 name, split so a repository grep for it finds nothing.
OLD_FORCE_VAR="AMPLIHACK_ALLOW_""RUNNER_DOWNGRADE"
unset AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE "$OLD_FORCE_VAR" ANCESTRY_OVERRIDE

# --- Newer: skip, but still validate and export -----------------------------
setup_case "$SHA_C"
run_installer
expect_status 0 "installer Newer: exits 0"
expect_cargo no "installer Newer: cargo is not called"
expect_out "$(short "$SHA_C")" "installer Newer: names the installed SHA"
expect_out "$(short "$PIN")" "installer Newer: names the pinned SHA"
expect_out "newer than pinned" "installer Newer: says the installed runner is newer"
expect_out "AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1" "installer Newer: names the force variable"
if grep -qx -- "--version" "$CASE/runner-log" 2>/dev/null &&
    grep -qx -- "--capabilities" "$CASE/runner-log" 2>/dev/null; then
    record_pass "installer Newer: --version and --capabilities still run"
else
    record_fail "installer Newer: kept runner was not validated"
fi
if grep -qxF "$CASE/runner-temp/recipe-runner/bin" "$CASE/github-path"; then
    record_pass "installer Newer: GITHUB_PATH export written"
else
    record_fail "installer Newer: GITHUB_PATH export missing"
fi
if grep -qxF "RECIPE_RUNNER_RS_PATH=$CASE/runner-temp/recipe-runner/bin/recipe-runner-rs" "$CASE/github-env"; then
    record_pass "installer Newer: GITHUB_ENV export written"
else
    record_fail "installer Newer: GITHUB_ENV export missing"
fi
last_line="$(awk 'NF { l = $0 } END { print l }' "$OUT")"
case "$last_line" in
    *"$(short "$SHA_C")"*"$(short "$PIN")"*Newer* | *"$(short "$PIN")"*"$(short "$SHA_C")"*Newer* | \
        *Newer*"$(short "$SHA_C")"*"$(short "$PIN")"* | *Newer*"$(short "$PIN")"*"$(short "$SHA_C")"* | \
        *"$(short "$SHA_C")"*Newer*"$(short "$PIN")"* | *"$(short "$PIN")"*Newer*"$(short "$SHA_C")"*)
        record_pass "installer Newer: final line names both SHAs and the class"
        ;;
    *)
        record_fail "installer Newer: final line '$last_line' lacks both SHAs and 'Newer'"
        ;;
esac

# --- Older and Unknown: install -------------------------------------------
setup_case "$SHA_A"
run_installer
expect_status 0 "installer Older: exits 0"
expect_cargo yes "installer Older: cargo is called"
if grep -qxF "$PIN" "$CASE/cargo-log" 2>/dev/null; then
    record_pass "installer Older: cargo installs the pinned revision"
else
    record_fail "installer Older: cargo was not given the pin"
fi
if grep -qx -- "--force" "$CASE/cargo-log" 2>/dev/null; then
    record_fail "installer Older: an upgrade must not pass --force"
else
    record_pass "installer Older: an upgrade does not pass --force"
fi

setup_case "-"
run_installer
expect_status 0 "installer Unknown (no receipt): exits 0"
expect_cargo yes "installer Unknown (no receipt): cargo is called"

# A receipt for a lookalike repository says nothing about the runner.
setup_case "$SHA_C" "$RUNNER_URL-evil"
run_installer
expect_cargo yes "installer lookalike receipt URL: treated as Unknown, cargo is called"

# A receipt without an executable binary is not trusted.
setup_case "$SHA_C"
chmod -x "$CASE/runner-temp/recipe-runner/bin/recipe-runner-rs"
run_installer
expect_cargo yes "installer non-executable binary: treated as Unknown, cargo is called"

# --- Equal: no cargo call ---------------------------------------------------
setup_case "$PIN"
run_installer
expect_status 0 "installer Equal: exits 0"
expect_cargo no "installer Equal: cargo is not called"
if grep -qx -- "--capabilities" "$CASE/runner-log" 2>/dev/null; then
    record_pass "installer Equal: existing binary is validated"
else
    record_fail "installer Equal: existing binary was not validated"
fi

# --- Diverged and Unverifiable: skip ----------------------------------------
setup_case "$SHA_D"
run_installer
expect_status 0 "installer Diverged: exits 0"
expect_cargo no "installer Diverged: cargo is not called"
expect_out "diverged from pinned" "installer Diverged: says the runner has diverged"

setup_case "$SHA_MISSING"
run_installer
expect_status 0 "installer Unverifiable: exits 0"
expect_cargo no "installer Unverifiable: cargo is not called"
expect_out_ci "could not verify" "installer Unverifiable: says the order could not be verified"

# --- Force ------------------------------------------------------------------
setup_case "$SHA_C"
run_installer --force
expect_status 0 "installer --force: exits 0"
expect_cargo yes "installer --force: cargo is called"
expect_out_ci "downgrade forced" "installer --force: reports the forced downgrade"
if grep -qx -- "--force" "$CASE/cargo-log" 2>/dev/null; then
    record_pass "installer --force: cargo receives --force as its own word"
else
    record_fail "installer --force: cargo did not receive --force"
fi

setup_case "$SHA_C"
AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1 run_installer
expect_cargo yes "installer AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1: cargo is called"
expect_out_ci "downgrade forced" "installer AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1: reports the forced downgrade"

for v in true yes " 1" "1 " 0 01 ""; do
    setup_case "$SHA_C"
    AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE="$v" run_installer
    expect_cargo no "installer AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE='$v' does not force"
done

# The pre-#1593 name was never released and has no alias: exporting it as 1
# must leave a Newer runner in place.
setup_case "$SHA_C"
export "$OLD_FORCE_VAR=1"
run_installer
unset "$OLD_FORCE_VAR"
expect_status 0 "installer old name $OLD_FORCE_VAR=1: exits 0"
expect_cargo no "installer old name $OLD_FORCE_VAR=1 does not force"
expect_out "newer than pinned" "installer old name: Newer runner is kept and reported"
expect_no_out "$OLD_FORCE_VAR" "installer old name: skip message does not name the old variable"

# Force over an Unverifiable runner also installs.
setup_case "$SHA_MISSING"
run_installer --force
expect_cargo yes "installer --force over Unverifiable: cargo is called"

# --- Arguments --------------------------------------------------------------
setup_case "$SHA_C"
run_installer --surprise
expect_status 2 "installer unknown argument exits 2"
expect_cargo no "installer unknown argument: cargo is not called"

setup_case "$SHA_C"
run_installer --force extra
expect_status 2 "installer extra positional argument exits 2"

# --- Hostile ancestry sources -----------------------------------------------
EXT_MARKER="$TMPROOT/ext-ran"
setup_case "$SHA_C"
ANCESTRY_OVERRIDE="ext::sh -c touch% $EXT_MARKER" run_installer
expect_cargo no "installer ext:: ancestry source: Unverifiable, cargo is not called"
expect_out_ci "could not verify" "installer ext:: ancestry source: reported as unverifiable"
if [ -e "$EXT_MARKER" ]; then
    record_fail "installer ext:: ancestry source ran a command"
else
    record_pass "installer ext:: ancestry source ran no command"
fi

setup_case "$SHA_C"
ANCESTRY_OVERRIDE="--upload-pack=touch $EXT_MARKER" run_installer
expect_cargo no "installer option-shaped ancestry source: Unverifiable, cargo is not called"
if [ -e "$EXT_MARKER" ]; then
    record_fail "installer option-shaped ancestry source ran a command"
else
    record_pass "installer option-shaped ancestry source ran no command"
fi

# A file:// URL to the fixture works as well as a path.
setup_case "$SHA_C"
ANCESTRY_OVERRIDE="file://$ANCESTRY" run_installer
expect_cargo no "installer file:// ancestry source: Newer is detected"
expect_out "newer than pinned" "installer file:// ancestry source: reports Newer"

# The ancestry override never changes the cargo install source.
setup_case "$SHA_A"
run_installer
if grep -qxF "$RUNNER_URL" "$CASE/cargo-log" 2>/dev/null; then
    record_pass "installer: cargo installs from the GitHub runner URL"
else
    record_fail "installer: cargo install source changed"
fi
if grep -qF "$ANCESTRY" "$CASE/cargo-log" 2>/dev/null; then
    record_fail "installer: ancestry repository leaked into cargo install"
else
    record_pass "installer: ancestry repository not passed to cargo"
fi

# ===========================================================================
# 3. claude-plugin/bin/install-runtime
# ===========================================================================
# rt_case <installed sha>: a plugin root with a fixture pin, a HOME, a Cargo
# home holding a receipt and runner, and stubs for cargo, amplihack, curl and
# node. The amplihack stub validates by logging its arguments.
rt_case() {
    case_n=$((case_n + 1))
    CASE="$TMPROOT/rt$case_n"
    mkdir -p "$CASE/root/claude-plugin" "$CASE/home" "$CASE/data" "$CASE/bin" \
        "$CASE/cargo/bin"
    printf '%s\n' "$PIN" >"$CASE/root/claude-plugin/recipe-runner.rev"
    OUT="$CASE/out"
    receipt "$CASE/cargo/.crates2.json" "$1"
    cat >"$CASE/cargo/bin/recipe-runner-rs" <<'STUB'
#!/bin/sh
echo '{"schema_version":1,"version":"0.5.0","capabilities":["codex_exec"]}'
STUB
    chmod +x "$CASE/cargo/bin/recipe-runner-rs"
    cat >"$CASE/bin/cargo" <<STUB
#!/bin/sh
for a in "\$@"; do printf '%s\n' "\$a"; done >> "$CASE/cargo-log"
echo "--" >> "$CASE/cargo-log"
rev=""
prev=""
for a in "\$@"; do
  [ "\$prev" = "--rev" ] && rev="\$a"
  prev="\$a"
done
printf '{"installs":{"recipe-runner-rs 0.5.0 (git+$RUNNER_URL#%s)":{"bins":["recipe-runner-rs"]}}}\n' "\$rev" > "$CASE/cargo/.crates2.json"
exit 0
STUB
    chmod +x "$CASE/bin/cargo"
    cat >"$CASE/bin/amplihack" <<STUB
#!/bin/sh
if [ "\$1" = --version ]; then echo "amplihack 9.9.9"; exit 0; fi
if [ "\$1" = internal ]; then
  case "\$*" in *--help*) exit 0 ;; esac
  printf '%s\n' "\$*" >> "$CASE/validator-log"
  exit 0
fi
exit 0
STUB
    chmod +x "$CASE/bin/amplihack"
    printf '#!/bin/sh\nexit 0\n' >"$CASE/bin/amplihack-hooks"
    printf '#!/bin/sh\nexit 7\n' >"$CASE/bin/curl"
    printf '#!/bin/sh\nexit 1\n' >"$CASE/bin/node"
    chmod +x "$CASE/bin/amplihack-hooks" "$CASE/bin/curl" "$CASE/bin/node"
}

run_runtime() {
    env -u RECIPE_RUNNER_RS_PATH -u AMPLIHACK_PLUGIN_INSTALL_LOCK \
        HOME="$CASE/home" CARGO_HOME="$CASE/cargo" \
        PATH="$CASE/bin:/usr/bin:/bin" \
        CLAUDE_PLUGIN_ROOT="$CASE/root" CLAUDE_PLUGIN_DATA="$CASE/data" \
        AMPLIHACK_NPM_VERSION=9.9.9 \
        RECIPE_RUNNER_ANCESTRY_REPO="$ANCESTRY" \
        sh "$INSTALL_RUNTIME" >"$OUT" 2>&1
    STATUS=$?
}

rt_case "$SHA_C"
run_runtime
expect_status 0 "install-runtime Newer: exits 0"
expect_cargo no "install-runtime Newer: cargo is not called"
expect_out "newer than pinned" "install-runtime Newer: reports the kept runner"
expect_out "$(short "$SHA_C")" "install-runtime Newer: names the installed SHA"
expect_out "$(short "$PIN")" "install-runtime Newer: names the pinned SHA"
if [ -s "$CASE/validator-log" ]; then
    record_pass "install-runtime Newer: kept runner is validated"
else
    record_fail "install-runtime Newer: kept runner was not validated"
fi
if grep -qF -- "--revision" "$CASE/validator-log" 2>/dev/null; then
    record_fail "install-runtime Newer: validation must not pass --revision for a kept runner"
else
    record_pass "install-runtime Newer: validation runs without --revision"
fi
if [ -f "$CASE/data/runtime.stamp" ]; then
    record_pass "install-runtime Newer: runtime is stamped"
else
    record_fail "install-runtime Newer: runtime was not stamped"
fi

# A kept runner that fails validation leaves runner_status non-zero.
rt_case "$SHA_C"
cat >"$CASE/bin/amplihack" <<STUB
#!/bin/sh
if [ "\$1" = --version ]; then echo "amplihack 9.9.9"; exit 0; fi
if [ "\$1" = internal ]; then
  case "\$*" in *--help*) exit 0 ;; esac
  exit 1
fi
exit 0
STUB
run_runtime
expect_cargo no "install-runtime Newer, failing validation: cargo is not called"
if [ "$STATUS" -ne 0 ] && [ ! -f "$CASE/data/runtime.stamp" ]; then
    record_pass "install-runtime Newer, failing validation: not stamped as success"
else
    record_fail "install-runtime Newer, failing validation: stamped or exited 0"
fi

rt_case "$SHA_A"
run_runtime
expect_cargo yes "install-runtime Older: cargo is called"
if grep -qxF "$PIN" "$CASE/cargo-log" 2>/dev/null; then
    record_pass "install-runtime Older: cargo installs the pin"
else
    record_fail "install-runtime Older: cargo was not given the pin"
fi
if grep -qF -- "--revision" "$CASE/validator-log" 2>/dev/null; then
    record_pass "install-runtime Older: installed runner validated against the pin"
else
    record_fail "install-runtime Older: --revision validation missing"
fi

rt_case "$SHA_D"
run_runtime
expect_cargo no "install-runtime Diverged: cargo is not called"
expect_out "diverged from pinned" "install-runtime Diverged: reports the kept runner"

rt_case "$SHA_C"
AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1 run_runtime
expect_cargo yes "install-runtime force: cargo is called"
expect_out_ci "downgrade forced" "install-runtime force: reports the forced downgrade"
if grep -qx -- "--force" "$CASE/cargo-log" 2>/dev/null; then
    record_pass "install-runtime force: cargo receives --force"
else
    record_fail "install-runtime force: cargo did not receive --force"
fi

rt_case "$SHA_C"
AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=true run_runtime
expect_cargo no "install-runtime AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=true does not force"

rt_case "$SHA_C"
export "$OLD_FORCE_VAR=1"
run_runtime
unset "$OLD_FORCE_VAR"
expect_cargo no "install-runtime old name $OLD_FORCE_VAR=1 does not force"
expect_out "newer than pinned" "install-runtime old name: Newer runner is kept and reported"

# No source, script or doc may still read or document the old name. A
# compatibility alias added later would reappear here and fail this check.
old_name_hits="$(grep -rnF "ALLOW_""RUNNER_DOWNGRADE" \
    "$REPO_ROOT/scripts" "$REPO_ROOT/claude-plugin/bin" "$REPO_ROOT/crates" \
    "$REPO_ROOT/docs" "$REPO_ROOT/.github/workflows" 2>/dev/null || true)"
old_anchor_hits="$(grep -rnF "amplihack_allow_""runner_downgrade" "$REPO_ROOT/docs" 2>/dev/null || true)"
if [ -z "$old_name_hits$old_anchor_hits" ]; then
    record_pass "old force variable name appears nowhere in scripts, crates, docs or workflows"
else
    record_fail "old force variable name still present:"
    printf '%s\n' "$old_name_hits" "$old_anchor_hits" | sed '/^$/d; s/^/    | /'
fi

# Both installers compare the new variable with the exact string 1.
for src in "$INSTALLER" "$INSTALL_RUNTIME"; do
    if grep -qE '"\$\{AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE:-\}" *==? *"?1"?( |;|\])' "$src"; then
        record_pass "$(basename "$src") compares AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE with exactly 1"
    else
        record_fail "$(basename "$src") lacks an exact comparison of AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE with 1"
    fi
done

# The existing cargo install line is kept verbatim (plugin manifest test).
if grep -qF 'cargo install --git "$runner_url" --rev "$rev" --locked' "$INSTALL_RUNTIME"; then
    record_pass "install-runtime keeps the pinned cargo install line"
else
    record_fail "install-runtime pinned cargo install line changed"
fi

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
