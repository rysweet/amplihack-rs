#!/usr/bin/env bash
# Build the lbug C++ library once per host and reuse it in every checkout.
#
# Why: the `lbug` crate's build script compiles about 2,000 C++ files with
# CMake inside each target directory. sccache does not see that build, and
# every worktree has its own target directory. So each new worktree, and each
# parallel agent, compiled the same unchanged library again: 48 C++ compilers
# at once on candide on 8 November 2026, with the load at 600 on 72 cores.
#
# What: the lbug crate already supports a prebuilt library. When
# LBUG_LIBRARY_DIR and LBUG_INCLUDE_DIR are both set, its build script skips
# CMake and links the static libraries found there. This script builds that
# library once, from the crate source cargo has already downloaded (nothing is
# copied into this repository), into a shared cache keyed by the lbug version,
# the target and the C++ compiler. It then prints the two variables.
#
#   scripts/lbug-prebuilt.sh env     ensure the cache, then print
#                                    `LBUG_LIBRARY_DIR=...` and `LBUG_INCLUDE_DIR=...`
#   scripts/lbug-prebuilt.sh build   ensure the cache only
#   scripts/lbug-prebuilt.sh ready   print the two variables only if the cache
#                                    is already built; never build (for hooks)
#   scripts/lbug-prebuilt.sh path    print the cache directory; build nothing
#
# The version comes from this checkout's Cargo.lock, so a checkout on a newer
# lbug gets its own cache entry and never links an older library. Concurrent
# callers wait on one lock, so N parallel builds cause one CMake build, not N.
# A failed build leaves no cache entry. Callers that get no output, or a
# non-zero exit, build lbug the normal way.
#
# Settings:
#   AMPLIHACK_LBUG_CACHE  cache root (default ${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/lbug)
#   AMPLIHACK_LBUG_JOBS   parallel C++ jobs for the one build (default: all cores)
#   AMPLIHACK_LBUG_PREBUILT=0  turn this off: print nothing and exit 0
set -euo pipefail

mode="${1:-env}"
[ "${AMPLIHACK_LBUG_PREBUILT:-1}" = "0" ] && exit 0

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
lock_file="$root/Cargo.lock"
[ -f "$lock_file" ] || { echo "lbug-prebuilt: no Cargo.lock in $root" >&2; exit 2; }

version="$(awk '/^\[\[package\]\]/{n=""} /^name = "lbug"$/{n=1} n && /^version = /{gsub(/"/,"",$3); print $3; exit}' "$lock_file")"
[ -n "$version" ] || { echo "lbug-prebuilt: lbug is not in Cargo.lock" >&2; exit 2; }

cxx="${CXX:-c++}"
target="$(rustc -vV 2>/dev/null | awk '/^host:/{print $2}')"
compiler="$("$cxx" --version 2>/dev/null | head -1)"
key="$(printf '%s\n%s\n%s\n' "$version" "$target" "$compiler" | sha256sum | cut -c1-12)"
cache_root="${AMPLIHACK_LBUG_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/lbug}"
dir="$cache_root/$version-$key"

print_env() {
  printf 'LBUG_LIBRARY_DIR=%s\n' "$dir/lib"
  printf 'LBUG_INCLUDE_DIR=%s\n' "$dir/include"
}

ready() { [ -f "$dir/READY" ] && [ -f "$dir/lib/liblbug.a" ] && [ -f "$dir/include/lbug.hpp" ]; }

if [ "$mode" = "path" ]; then echo "$dir"; exit 0; fi

build() {
  ready && return 0
  mkdir -p "$cache_root"
  exec 8>"$dir.lock"
  flock 8
  ready && return 0

  local src=""
  for d in "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/"lbug-$version"; do
    [ -f "$d/lbug-src/CMakeLists.txt" ] && { src="$d/lbug-src"; break; }
  done
  if [ -z "$src" ]; then
    (cd "$root" && cargo fetch --locked >/dev/null 2>&1) || true
    for d in "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/"lbug-$version"; do
      [ -f "$d/lbug-src/CMakeLists.txt" ] && { src="$d/lbug-src"; break; }
    done
  fi
  [ -n "$src" ] || { echo "lbug-prebuilt: lbug-$version source not found under the cargo registry" >&2; return 2; }

  local work="$dir.tmp.$$"
  rm -rf -- "$work"
  mkdir -p "$work/lib" "$work/include"
  local gen=()
  command -v ninja >/dev/null 2>&1 && gen=(-G Ninja)
  echo "lbug-prebuilt: building lbug $version once into $dir (later builds reuse it)" >&2
  # The same switches as the crate's build script, plus position-independent
  # code for linking into Rust executables, and a release build.
  cmake -S "$src" -B "$work/build" "${gen[@]}" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
    -DCMAKE_CXX_COMPILER="$cxx" \
    -DBUILD_SHELL=OFF \
    -DBUILD_SINGLE_FILE_HEADER=OFF \
    -DAUTO_UPDATE_GRAMMAR=OFF >"$work/cmake.log" 2>&1 \
    || { echo "lbug-prebuilt: cmake configure failed, see $work/cmake.log" >&2; return 1; }
  cmake --build "$work/build" -j "${AMPLIHACK_LBUG_JOBS:-$(nproc)}" >>"$work/cmake.log" 2>&1 \
    || { echo "lbug-prebuilt: cmake build failed, see $work/cmake.log" >&2; return 1; }

  # The crate links liblbug.a plus these static libraries from LBUG_LIBRARY_DIR.
  local lib missing=0
  for lib in lbug utf8proc antlr4_cypher antlr4_runtime re2 fastpfor parquet thrift snappy zstd \
             miniz mbedtls brotlidec brotlicommon lz4 roaring_bitmap simsimd yyjson; do
    local a
    a="$(find "$work/build" -name "lib$lib.a" -print -quit)"
    if [ -n "$a" ]; then cp "$a" "$work/lib/"; else echo "lbug-prebuilt: lib$lib.a not built" >&2; missing=1; fi
  done
  [ "$missing" -eq 0 ] || return 1

  # Headers. The crate's prebuilt route adds one include directory and includes
  # `<lbug.hpp>`, lbug's single-file header. The published crate leaves out the
  # script that generates that header, so the cache gathers the headers the
  # crate's bundled build uses into one directory instead (the same six
  # directories as its build script), and writes an lbug.hpp that includes the
  # same headers as the bundled branch of include/lbug_rs.h and lbug_arrow.h.
  copy_headers() { # copy_headers <from> <to>: every C/C++ header, keeping paths
    [ -d "$1" ] || return 0
    (cd "$1" && find . -type f \( -name '*.h' -o -name '*.hpp' -o -name '*.inc' -o -name '*.ipp' \) -print0 |
      xargs -0 -r cp --parents -t "$2")
  }
  copy_headers "$src/src/include" "$work/include"
  copy_headers "$work/build/src" "$work/include"
  copy_headers "$work/build/src/include" "$work/include"
  copy_headers "$src/third_party/nlohmann_json" "$work/include"
  copy_headers "$src/third_party/fastpfor" "$work/include"
  copy_headers "$src/third_party/alp/include" "$work/include"
  cat >"$work/include/lbug.hpp" <<'EOF'
// Written by amplihack's scripts/lbug-prebuilt.sh: the headers that the lbug
// crate's bundled build includes (include/lbug_rs.h, include/lbug_arrow.h).
#pragma once
#include "common/type_utils.h"
#include "common/types/int128_t.h"
#include "common/types/types.h"
#include "common/types/value/nested.h"
#include "common/types/value/node.h"
#include "common/types/value/recursive_rel.h"
#include "common/types/value/rel.h"
#include "common/types/value/value.h"
#include "main/lbug.h"
#include "storage/storage_version_info.h"
EOF

  printf 'lbug %s\ntarget %s\ncompiler %s\nsource %s\n' "$version" "$target" "$compiler" "$src" >"$work/READY"
  rm -rf -- "$work/build"
  rm -rf -- "$dir"
  mv "$work" "$dir"
}

case "$mode" in
  build) build ;;
  env) build && print_env ;;
  ready) if ready; then print_env; fi ;;
  *) echo "usage: scripts/lbug-prebuilt.sh [env|build|ready|path]" >&2; exit 2 ;;
esac
