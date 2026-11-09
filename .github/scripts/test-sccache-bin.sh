#!/usr/bin/env bash
# Behaviour tests for .github/scripts/sccache-bin.sh (issue #2105).
#
#   bash .github/scripts/test-sccache-bin.sh
#
# Each case runs the script in a fresh scratch directory with its own
# GITHUB_ENV, GITHUB_PATH, GITHUB_OUTPUT and RUNNER_TEMP, and checks what it
# wrote there. No network, no sleep, no timing.
#
# Cases that need the genuine v0.19.0 binary (cache hit, symlink, save) use
# SCCACHE_TEST_BINARY, or else `command -v sccache`, but only when its sha256
# equals the pinned value for this host. Otherwise they print SKIP. Set
# SCCACHE_TEST_REQUIRE_BINARY=1 to turn a SKIP into a failure.
#
# Contract under test (see docs/reference/ci-pipeline.md, "sccache binary"):
#   inputs  : argv[1] = restore | finish; env SIMULATE, KEY_OSARCH, BIN_DIR,
#             RUNNER_TEMP, SCCACHE_PATH, GITHUB_ENV, GITHUB_PATH, GITHUB_OUTPUT
#   restore : writes exactly one hit=true|false to GITHUB_OUTPUT; on hit it
#             adds $RUNNER_TEMP/sccache-verified to GITHUB_PATH and writes
#             SCCACHE_PATH=$RUNNER_TEMP/sccache-verified/sccache to GITHUB_ENV
#   finish  : writes exactly one save=true|false; with no usable binary it
#             writes RUSTC_WRAPPER= to GITHUB_ENV and prints the fallback line
#             once; a usable binary that cannot be staged gives save=false and
#             leaves the wrapper on
#   restore : a cached file that fails verification prints one ::warning::
#   a binary is never executed before its sha256 matches the pinned value
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
SCRIPT="$HERE/sccache-bin.sh"

LINUX_X64_SHA=0b44834be857e21b026f7db49a7762023fedb5eb5ed60725a66f698d8d79a34b
MACOS_ARM64_SHA=d81ba04c41e6d772f7ecc83fe8f26f2f8b25f2cf3a730049c926c7592c819290
WARNING='::warning::sccache binary unavailable (cache miss and download failed); building without RUSTC_WRAPPER'

PASS=0
FAIL=0
SKIP=0
CURRENT=""

pass() { PASS=$((PASS + 1)); echo "ok   - $CURRENT"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL - $CURRENT: $*"; }
skip() {
  if [ "${SCCACHE_TEST_REQUIRE_BINARY:-}" = 1 ]; then
    fail "SKIP not allowed (SCCACHE_TEST_REQUIRE_BINARY=1): $*"
  else
    SKIP=$((SKIP + 1)); echo "SKIP - $CURRENT: $*"
  fi
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" 2>/dev/null | cut -d' ' -f1
  else
    shasum -a 256 "$1" 2>/dev/null | cut -d' ' -f1
  fi
}

# Host runner type in the form the composite action passes as KEY_OSARCH
# ("${{ runner.os }}-${{ runner.arch }}").
host_osarch() {
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) echo Linux-X64 ;;
    Linux-aarch64) echo Linux-ARM64 ;;
    Darwin-arm64) echo macOS-ARM64 ;;
    Darwin-x86_64) echo macOS-X64 ;;
    *) echo unknown ;;
  esac
}

HOST_OSARCH=$(host_osarch)
case "$HOST_OSARCH" in
  Linux-X64) HOST_SHA=$LINUX_X64_SHA ;;
  macOS-ARM64) HOST_SHA=$MACOS_ARM64_SHA ;;
  *) HOST_SHA="" ;;
esac

# The genuine binary for this host, if one is available.
REAL=""
cand=${SCCACHE_TEST_BINARY:-$(command -v sccache 2>/dev/null || true)}
if [ -n "$cand" ] && [ -f "$cand" ] && [ -n "$HOST_SHA" ] \
  && [ "$(sha256_of "$cand")" = "$HOST_SHA" ]; then
  REAL=$cand
fi

ROOT=$(mktemp -d "${TMPDIR:-/tmp}/sccache-bin-test.XXXXXX")
trap 'rm -rf "$ROOT"' EXIT

# A PATH that has every tool from /usr/bin and /bin except sccache (and, with
# NOHASH=1, except sha256sum and shasum), so `command -v sccache` finds only
# what a case puts there.
make_path_dir() {
  local dir=$1 nohash=${2:-0} d f base
  mkdir -p "$dir"
  for d in /usr/local/bin /usr/bin /bin /usr/sbin /sbin; do
    [ -d "$d" ] || continue
    for f in "$d"/*; do
      base=${f##*/}
      [ -e "$dir/$base" ] && continue
      case "$base" in
        sccache) continue ;;
        sha256sum | shasum) [ "$nohash" = 1 ] && continue ;;
      esac
      ln -s "$f" "$dir/$base" 2>/dev/null || true
    done
  done
}
CLEAN_PATH="$ROOT/path-clean"
NOHASH_PATH="$ROOT/path-nohash"
make_path_dir "$CLEAN_PATH" 0
make_path_dir "$NOHASH_PATH" 1

# A fake "sccache" that would pass a version check if it were ever run, and
# leaves a marker when it is. Its hash never matches the pinned values.
make_fake() {
  local path=$1 marker=$2
  mkdir -p "$(dirname "$path")"
  cat >"$path" <<EOF
#!/bin/sh
: > "$marker"
echo "sccache 0.19.0"
EOF
  chmod 0755 "$path"
}

# new_case NAME: fresh scratch directory and env files; sets CASE_DIR.
new_case() {
  CURRENT=$1
  CASE_DIR="$ROOT/case-$((PASS + FAIL + SKIP))"
  mkdir -p "$CASE_DIR/runner-temp" "$CASE_DIR/runner-temp/sccache-bin"
  : >"$CASE_DIR/env"
  : >"$CASE_DIR/path"
  : >"$CASE_DIR/output"
}

# run_script ARG [VAR=VALUE ...]: runs the script with a controlled environment.
# Sets RC, OUT (stdout) and ERR (stderr).
run_script() {
  local arg=$1
  shift
  # $arg is unquoted on purpose: an empty value means "no argument".
  # shellcheck disable=SC2086
  env -i \
    HOME="$CASE_DIR" \
    PATH="$CLEAN_PATH" \
    RUNNER_TEMP="$CASE_DIR/runner-temp" \
    BIN_DIR="$CASE_DIR/runner-temp/sccache-bin" \
    KEY_OSARCH="$HOST_OSARCH" \
    SIMULATE=false \
    GITHUB_ENV="$CASE_DIR/env" \
    GITHUB_PATH="$CASE_DIR/path" \
    GITHUB_OUTPUT="$CASE_DIR/output" \
    "$@" \
    bash "$SCRIPT" $arg >"$CASE_DIR/stdout" 2>"$CASE_DIR/stderr"
  RC=$?
  OUT=$(cat "$CASE_DIR/stdout")
  ERR=$(cat "$CASE_DIR/stderr")
}

count_lines() { grep -c -x -F -- "$2" "$1" 2>/dev/null || true; }
count_prefix() { grep -c -- "^$2" "$1" 2>/dev/null || true; }

VERIFIED_REL=runner-temp/sccache-verified

# Shared checks -------------------------------------------------------------

expect_restore_miss() {
  local why=""
  [ "$RC" = 0 ] || why+=" exit=$RC;"
  [ "$(count_prefix "$CASE_DIR/output" 'hit=')" = 1 ] || why+=" hit= lines != 1;"
  [ "$(count_lines "$CASE_DIR/output" 'hit=false')" = 1 ] || why+=" no hit=false;"
  [ ! -s "$CASE_DIR/path" ] || why+=" wrote GITHUB_PATH;"
  grep -q '^SCCACHE_PATH=' "$CASE_DIR/env" && why+=" wrote SCCACHE_PATH;"
  grep -q '^RUSTC_WRAPPER=' "$CASE_DIR/env" && why+=" restore must not touch RUSTC_WRAPPER;"
  if [ -d "$CASE_DIR/$VERIFIED_REL" ] && [ -n "$(ls -A "$CASE_DIR/$VERIFIED_REL")" ]; then
    why+=" verified dir not empty;"
  fi
  echo "$why"
}

expect_fallback() {
  local why=""
  [ "$RC" = 0 ] || why+=" exit=$RC;"
  [ "$(count_prefix "$CASE_DIR/output" 'save=')" = 1 ] || why+=" save= lines != 1;"
  [ "$(count_lines "$CASE_DIR/output" 'save=false')" = 1 ] || why+=" no save=false;"
  [ "$(count_lines "$CASE_DIR/env" 'RUSTC_WRAPPER=')" = 1 ] || why+=" no RUSTC_WRAPPER= line;"
  [ "$(count_lines "$CASE_DIR/stdout" "$WARNING")" = 1 ] || why+=" warning line not printed exactly once on stdout;"
  [ "$(grep -c '::warning::' "$CASE_DIR/stdout" "$CASE_DIR/stderr" | awk -F: '{s+=$NF} END {print s+0}')" = 1 ] \
    || why+=" more than one ::warning:: line;"
  echo "$why"
}

no_secrets_in_log() {
  # Nothing read from the binary or a hash value goes to the log.
  local why=""
  case "$OUT$ERR" in
    *"$LINUX_X64_SHA"* | *"$MACOS_ARM64_SHA"*) why+=" hash value in log;" ;;
  esac
  case "$OUT$ERR" in
    *"sccache 0.19.0"*) why+=" --version output in log;" ;;
  esac
  echo "$why"
}

verdict() { if [ -z "$1" ]; then pass; else fail "$1"; fi; }

# The script must exist; without this, a missing script would pass the
# "exits non-zero" cases below.
CURRENT="sccache-bin.sh exists and passes bash -n"
if [ -f "$SCRIPT" ] && bash -n "$SCRIPT" 2>/dev/null; then pass; else fail "$SCRIPT missing or not valid bash"; fi

# Argument handling ---------------------------------------------------------

new_case "unknown phase argument exits non-zero and writes nothing"
run_script bogus
why=""
[ -f "$SCRIPT" ] || why+=" script missing;"
[ "$RC" != 0 ] || why+=" exit=0;"
[ "$RC" != 127 ] || why+=" exit=127 (script not run);"
[ ! -s "$CASE_DIR/output" ] || why+=" wrote GITHUB_OUTPUT;"
[ ! -s "$CASE_DIR/env" ] || why+=" wrote GITHUB_ENV;"
[ ! -s "$CASE_DIR/path" ] || why+=" wrote GITHUB_PATH;"
[ -n "$ERR$OUT" ] || why+=" no error message;"
verdict "$why"

new_case "missing phase argument exits non-zero"
run_script ""
if [ -f "$SCRIPT" ] && [ "$RC" != 0 ] && [ "$RC" != 127 ] && [ ! -s "$CASE_DIR/output" ]; then pass; else fail "exit=$RC"; fi

# Restore phase -------------------------------------------------------------

new_case "restore: empty cache directory is a miss"
run_script restore
verdict "$(expect_restore_miss)"

new_case "restore: tampered binary is a miss and is never executed"
marker="$CASE_DIR/executed"
make_fake "$CASE_DIR/runner-temp/sccache-bin/sccache" "$marker"
run_script restore
why=$(expect_restore_miss)
[ ! -e "$marker" ] || why+=" tampered binary was executed;"
[ "$(grep -c '^::warning::cached sccache binary failed verification' "$CASE_DIR/stdout")" = 1 ] \
  || why+=" no verification-failure warning;"
verdict "$why"

new_case "restore: tampered binary on an unknown runner type is a miss, not executed"
marker="$CASE_DIR/executed"
make_fake "$CASE_DIR/runner-temp/sccache-bin/sccache" "$marker"
run_script restore KEY_OSARCH=Windows-X64
why=$(expect_restore_miss)
[ ! -e "$marker" ] || why+=" binary was executed;"
verdict "$why"

new_case "restore: SIMULATE=true is a miss even with a binary present"
marker="$CASE_DIR/executed"
make_fake "$CASE_DIR/runner-temp/sccache-bin/sccache" "$marker"
run_script restore SIMULATE=true
why=$(expect_restore_miss)
[ ! -e "$marker" ] || why+=" binary was executed;"
verdict "$why"

new_case "restore: directory in place of the binary is a miss"
mkdir -p "$CASE_DIR/runner-temp/sccache-bin/sccache"
run_script restore
verdict "$(expect_restore_miss)"

new_case "restore: stale files in the verified directory are removed on a miss"
mkdir -p "$CASE_DIR/$VERIFIED_REL"
make_fake "$CASE_DIR/$VERIFIED_REL/sccache" "$CASE_DIR/executed"
make_fake "$CASE_DIR/runner-temp/sccache-bin/sccache" "$CASE_DIR/executed"
run_script restore
verdict "$(expect_restore_miss)"

if [ -n "$REAL" ]; then
  new_case "restore: genuine binary is a hit, used from a private verified copy"
  cp "$REAL" "$CASE_DIR/runner-temp/sccache-bin/sccache"
  chmod 0644 "$CASE_DIR/runner-temp/sccache-bin/sccache"
  run_script restore
  verified="$CASE_DIR/$VERIFIED_REL"
  why=""
  [ "$RC" = 0 ] || why+=" exit=$RC;"
  [ "$(count_prefix "$CASE_DIR/output" 'hit=')" = 1 ] || why+=" hit= lines != 1;"
  [ "$(count_lines "$CASE_DIR/output" 'hit=true')" = 1 ] || why+=" no hit=true;"
  [ "$(cat "$CASE_DIR/path")" = "$verified" ] || why+=" GITHUB_PATH is not exactly the verified dir;"
  [ "$(count_lines "$CASE_DIR/env" "SCCACHE_PATH=$verified/sccache")" = 1 ] || why+=" SCCACHE_PATH not the verified copy;"
  grep -q '^RUSTC_WRAPPER=' "$CASE_DIR/env" && why+=" hit must not touch RUSTC_WRAPPER;"
  [ -f "$verified/sccache" ] && [ ! -L "$verified/sccache" ] || why+=" verified copy missing;"
  [ -x "$verified/sccache" ] || why+=" verified copy not executable;"
  [ "$(sha256_of "$verified/sccache")" = "$HOST_SHA" ] || why+=" verified copy hash differs;"
  [ "$(ls -A "$verified" 2>/dev/null)" = sccache ] || why+=" verified dir holds more than sccache;"
  grep -qF "$CASE_DIR/runner-temp/sccache-bin" "$CASE_DIR/path" && why+=" cache dir put on PATH;"
  why+=$(no_secrets_in_log)
  verdict "$why"

  new_case "restore: genuine binary on a runner type with no pinned hash is a miss"
  cp "$REAL" "$CASE_DIR/runner-temp/sccache-bin/sccache"
  run_script restore KEY_OSARCH=Windows-X64
  verdict "$(expect_restore_miss)"

  new_case "restore: genuine binary under the other runner type's hash is a miss"
  cp "$REAL" "$CASE_DIR/runner-temp/sccache-bin/sccache"
  if [ "$HOST_OSARCH" = Linux-X64 ]; then other=macOS-ARM64; else other=Linux-X64; fi
  run_script restore KEY_OSARCH="$other"
  verdict "$(expect_restore_miss)"

  new_case "restore: symlink to the genuine binary is a miss"
  ln -s "$REAL" "$CASE_DIR/runner-temp/sccache-bin/sccache"
  run_script restore
  verdict "$(expect_restore_miss)"

  new_case "restore: no sha256 tool on PATH is a miss, not a failure"
  cp "$REAL" "$CASE_DIR/runner-temp/sccache-bin/sccache"
  run_script restore PATH="$NOHASH_PATH"
  verdict "$(expect_restore_miss)"
else
  for name in "restore: genuine binary is a hit" \
    "restore: genuine binary on a runner type with no pinned hash" \
    "restore: genuine binary under the other runner type's hash" \
    "restore: symlink to the genuine binary" \
    "restore: no sha256 tool on PATH"; do
    CURRENT=$name
    skip "no genuine v0.19.0 binary for $HOST_OSARCH"
  done
fi

# Finish phase --------------------------------------------------------------

new_case "finish: no SCCACHE_PATH and no sccache on PATH falls back"
run_script finish
why=$(expect_fallback)
[ -z "$(ls -A "$CASE_DIR/runner-temp/sccache-bin")" ] || why+=" wrote into BIN_DIR;"
verdict "$why"

new_case "finish: SIMULATE=true falls back"
run_script finish SIMULATE=true
verdict "$(expect_fallback)"

new_case "finish: tampered SCCACHE_PATH falls back and is never executed"
marker="$CASE_DIR/executed"
make_fake "$CASE_DIR/download/sccache" "$marker"
run_script finish SCCACHE_PATH="$CASE_DIR/download/sccache"
why=$(expect_fallback)
[ ! -e "$marker" ] || why+=" tampered binary was executed;"
[ ! -e "$CASE_DIR/runner-temp/sccache-bin/sccache" ] || why+=" tampered binary staged for save;"
verdict "$why"

new_case "finish: tampered sccache on PATH falls back and is never executed"
marker="$CASE_DIR/executed"
make_fake "$CASE_DIR/download/sccache" "$marker"
run_script finish PATH="$CASE_DIR/download:$CLEAN_PATH"
why=$(expect_fallback)
[ ! -e "$marker" ] || why+=" tampered binary was executed;"
verdict "$why"

new_case "finish: SCCACHE_PATH pointing at nothing falls back"
run_script finish SCCACHE_PATH="$CASE_DIR/does-not-exist/sccache"
verdict "$(expect_fallback)"

if [ -n "$REAL" ]; then
  new_case "finish: genuine SCCACHE_PATH is staged alone in BIN_DIR, save=true"
  mkdir -p "$CASE_DIR/download"
  cp "$REAL" "$CASE_DIR/download/sccache"
  : >"$CASE_DIR/runner-temp/sccache-bin/leftover"
  run_script finish SCCACHE_PATH="$CASE_DIR/download/sccache"
  bin="$CASE_DIR/runner-temp/sccache-bin"
  why=""
  [ "$RC" = 0 ] || why+=" exit=$RC;"
  [ "$(count_prefix "$CASE_DIR/output" 'save=')" = 1 ] || why+=" save= lines != 1;"
  [ "$(count_lines "$CASE_DIR/output" 'save=true')" = 1 ] || why+=" no save=true;"
  grep -q '^RUSTC_WRAPPER=' "$CASE_DIR/env" && why+=" wrapper turned off;"
  grep -q '::warning::' "$CASE_DIR/stdout" && why+=" printed a warning;"
  [ "$(ls -A "$bin" 2>/dev/null)" = sccache ] || why+=" BIN_DIR holds more than sccache;"
  [ -f "$bin/sccache" ] && [ ! -L "$bin/sccache" ] || why+=" staged file missing or a link;"
  [ -x "$bin/sccache" ] || why+=" staged file not executable;"
  [ "$(sha256_of "$bin/sccache")" = "$HOST_SHA" ] || why+=" staged hash differs;"
  why+=$(no_secrets_in_log)
  verdict "$why"

  new_case "finish: genuine sccache found by command -v, save=true"
  mkdir -p "$CASE_DIR/download"
  cp "$REAL" "$CASE_DIR/download/sccache"
  run_script finish PATH="$CASE_DIR/download:$CLEAN_PATH"
  why=""
  [ "$(count_lines "$CASE_DIR/output" 'save=true')" = 1 ] || why+=" no save=true;"
  grep -q '^RUSTC_WRAPPER=' "$CASE_DIR/env" && why+=" wrapper turned off;"
  verdict "$why"

  new_case "finish: SIMULATE=true falls back even with a genuine sccache on PATH"
  mkdir -p "$CASE_DIR/download"
  cp "$REAL" "$CASE_DIR/download/sccache"
  run_script finish SIMULATE=true PATH="$CASE_DIR/download:$CLEAN_PATH" SCCACHE_PATH="$CASE_DIR/download/sccache"
  why=$(expect_fallback)
  [ ! -e "$CASE_DIR/runner-temp/sccache-bin/sccache" ] || why+=" staged for save while simulating;"
  verdict "$why"

  new_case "finish: genuine binary on a runner type with no pinned hash falls back"
  mkdir -p "$CASE_DIR/download"
  cp "$REAL" "$CASE_DIR/download/sccache"
  run_script finish KEY_OSARCH=Windows-X64 SCCACHE_PATH="$CASE_DIR/download/sccache"
  verdict "$(expect_fallback)"

  new_case "finish: genuine binary that cannot be staged keeps the wrapper, save=false"
  mkdir -p "$CASE_DIR/download"
  cp "$REAL" "$CASE_DIR/download/sccache"
  : >"$CASE_DIR/blocker"
  run_script finish SCCACHE_PATH="$CASE_DIR/download/sccache" BIN_DIR="$CASE_DIR/blocker/sccache-bin"
  why=""
  [ "$RC" = 0 ] || why+=" exit=$RC;"
  [ "$(count_prefix "$CASE_DIR/output" 'save=')" = 1 ] || why+=" save= lines != 1;"
  [ "$(count_lines "$CASE_DIR/output" 'save=false')" = 1 ] || why+=" no save=false;"
  grep -q '^RUSTC_WRAPPER=' "$CASE_DIR/env" && why+=" wrapper turned off;"
  grep -q '::warning::' "$CASE_DIR/stdout" && why+=" printed the fallback warning;"
  verdict "$why"

  new_case "finish: symlinked SCCACHE_PATH falls back"
  mkdir -p "$CASE_DIR/download"
  ln -s "$REAL" "$CASE_DIR/download/sccache"
  run_script finish SCCACHE_PATH="$CASE_DIR/download/sccache"
  verdict "$(expect_fallback)"
else
  for name in "finish: genuine SCCACHE_PATH is staged" \
    "finish: genuine sccache found by command -v" \
    "finish: SIMULATE=true with a genuine sccache on PATH" \
    "finish: genuine binary on a runner type with no pinned hash" \
    "finish: genuine binary that cannot be staged" \
    "finish: symlinked SCCACHE_PATH"; do
    CURRENT=$name
    skip "no genuine v0.19.0 binary for $HOST_OSARCH"
  done
fi

echo
echo "sccache-bin.sh: $PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" = 0 ]
