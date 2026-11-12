#!/usr/bin/env bash
# Get the sccache binary from the Actions cache, or build without it
# (issue #2105). Called by .github/actions/sccache-bin; see "sccache binary"
# in docs/reference/ci-pipeline.md.
#
#   sccache-bin.sh restore   check a restored cache entry; output hit=true|false
#   sccache-bin.sh finish    after a miss: keep a downloaded binary (save=true;
#                            save=false if it cannot be staged) or turn
#                            RUSTC_WRAPPER off (save=false + warning)
#
# Environment (set by the composite action through env:, never inlined):
#   BIN_DIR       cache directory, ${{ runner.temp }}/sccache-bin
#   KEY_OSARCH    "${{ runner.os }}-${{ runner.arch }}", e.g. Linux-X64
#   SIMULATE      the literal "true" forces the missing-binary path
#   SCCACHE_PATH  set by sccache-action after a successful download
#
# No `set -e`: every branch must write exactly one output value, and a failed
# check means "missing", never a failed job. No retry, sleep or timeout: a
# missing binary is handled by building without the wrapper (Ryan's timing
# rule).
set -u

readonly WANT_VERSION='sccache 0.19.0'
readonly WARNING='::warning::sccache binary unavailable (cache miss and download failed); building without RUSTC_WRAPPER'

# sha256 of the `sccache` binary itself (not the tarball) inside each release
# asset of mozilla/sccache tag v0.19.0, the assets sccache-action v0.0.10
# downloads on each runner type:
#   Linux-X64    sccache-v0.19.0-x86_64-unknown-linux-musl.tar.gz
#   macOS-ARM64  sccache-v0.19.0-aarch64-apple-darwin.tar.gz
# Each tarball was checked against mozilla's published .sha256 file before the
# binary inside was hashed (2026-11-08). Any other runner type has no pinned
# hash, so its binary counts as missing. A version change must update, in the
# same commit: these hashes, WANT_VERSION, the cache key in
# .github/actions/sccache-bin, the `version:` input of every Setup sccache step
# in ci.yml and release.yml, and the copies of the hashes (LINUX_X64_SHA,
# MACOS_ARM64_SHA) in .github/scripts/test-sccache-bin.sh. Search for the old
# version string to find every place it is written.
pinned_sha() {
  case "${KEY_OSARCH:-}" in
    Linux-X64) printf '%s' 0b44834be857e21b026f7db49a7762023fedb5eb5ed60725a66f698d8d79a34b ;;
    macOS-ARM64) printf '%s' d81ba04c41e6d772f7ecc83fe8f26f2f8b25f2cf3a730049c926c7592c819290 ;;
    *) printf '' ;;
  esac
}

# Prints the sha256 of $1, or nothing when no hash tool exists (macOS runners
# have shasum but not always sha256sum).
sha_of() {
  local out=""
  if command -v sha256sum >/dev/null 2>&1; then
    out=$(sha256sum -- "$1" 2>/dev/null)
  elif command -v shasum >/dev/null 2>&1; then
    out=$(shasum -a 256 -- "$1" 2>/dev/null)
  fi
  printf '%s' "${out%% *}"
}

# usable PATH: true only if PATH is the pinned v0.19.0 binary.
# SECURITY: the order is fixed and joined by &&. The file is never executed
# until a pinned hash exists, it is a regular file (not a symlink) and its
# sha256 matches. Only then is `--version` run. An empty hash never matches.
usable() {
  local want got
  want=$(pinned_sha)
  [ -n "$want" ] &&
    [ -f "$1" ] && [ ! -L "$1" ] &&
    got=$(sha_of "$1") && [ -n "$got" ] && [ "$got" = "$want" ] &&
    [ "$("$1" --version 2>/dev/null)" = "$WANT_VERSION" ]
}

out() { printf '%s\n' "$1" >>"$GITHUB_OUTPUT"; }

# Restore phase. The cache directory never goes on PATH: the file is copied
# into a fresh private directory and every check runs on that copy, so nothing
# can change it between the check and its use.
do_restore() {
  local verified="$RUNNER_TEMP/sccache-verified"
  rm -rf -- "$verified"
  if [ "${SIMULATE:-}" = true ] || [ ! -f "$BIN_DIR/sccache" ] || [ -L "$BIN_DIR/sccache" ]; then
    out hit=false
    return 0
  fi
  if mkdir -p -- "$verified" &&
    install -m 0755 -- "$BIN_DIR/sccache" "$verified/sccache" &&
    usable "$verified/sccache"; then
    printf '%s\n' "$verified" >>"$GITHUB_PATH"
    printf '%s\n' "SCCACHE_PATH=$verified/sccache" >>"$GITHUB_ENV"
    out hit=true
  else
    # The key is immutable, so a bad entry is never replaced by a later save:
    # every run would download again until someone deletes the entry.
    printf '%s\n' "::warning::cached sccache binary failed verification; delete the Actions cache entry sccache-bin-v0.19.0-${KEY_OSARCH:-} so a verified binary can be saved"
    rm -rf -- "$verified"
    out hit=false
  fi
}

# Finish phase, run only after a miss. Keeps a binary that passes the same
# checks, or turns the wrapper off for the rest of the job.
do_finish() {
  local cand=""
  if [ "${SIMULATE:-}" != true ]; then
    cand=${SCCACHE_PATH:-$(command -v sccache 2>/dev/null)}
  fi
  if [ -n "$cand" ] && usable "$cand"; then
    if rm -rf -- "$BIN_DIR" && mkdir -p -- "$BIN_DIR" &&
      install -m 0755 -- "$cand" "$BIN_DIR/sccache" &&
      usable "$BIN_DIR/sccache"; then
      out save=true
      return 0
    fi
    # The binary works, so keep the wrapper on; only the save is skipped.
    echo "Could not stage the downloaded sccache binary for the cache; not saving it."
    rm -rf -- "$BIN_DIR" 2>/dev/null
    out save=false
    return 0
  fi
  printf '%s\n' "RUSTC_WRAPPER=" >>"$GITHUB_ENV"
  out save=false
  printf '%s\n' "$WARNING"
}

case "${1:-}" in
  restore) do_restore ;;
  finish) do_finish ;;
  *)
    echo "usage: sccache-bin.sh restore|finish (got '${1:-}')" >&2
    exit 2
    ;;
esac
