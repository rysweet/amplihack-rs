#!/usr/bin/env bash
# Run only inside the assigned admitted detached native runner.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/amplihack-lbug-native.XXXXXX")"
# macOS /tmp and /var can be aliases; managed roots use canonical ancestors.
work="$(cd "$work" && pwd -P)"
printf 'Native evidence: %s\n' "$work"
git -C "$repo" rev-parse HEAD > "$work/commit.txt"
export AMPLIHACK_LBUG_CACHE_DIR="$work/cache"
export AMPLIHACK_LBUG_CACHE=auto
export CARGO_BUILD_JOBS=1
export CMAKE_BUILD_PARALLEL_LEVEL=1
export RUSTUP_AUTO_INSTALL=0
unset LBUG_SHARED LBUG_LIBRARY_DIR LBUG_INCLUDE_DIR
export LBUG_NATIVE_REAL_CXX="$(command -v "${CXX:-c++}")"
"$LBUG_NATIVE_REAL_CXX" --version > "$work/compiler.txt"
mkdir "$work/tools" "$work/trace-a" "$work/trace-b" "$work/tripwire"
cat > "$work/tools/c++" <<'SH'
#!/usr/bin/env bash
set -eu
trace="$(mktemp "$LBUG_NATIVE_TRACE/invocation.XXXXXX")"
printf '%s\0' "$PWD" "$@" > "$trace"
exec "$LBUG_NATIVE_REAL_CXX" "$@"
SH
cat > "$work/tripwire/cmake" <<'SH'
#!/usr/bin/env bash
echo 'Unexpected CMake invocation during lbug cache hit' >&2
exit 97
SH
chmod 700 "$work/tools/c++" "$work/tripwire/cmake"
export CXX="$work/tools/c++"
target="$(rustc -vV | sed -n 's/^host: //p')"
# A pre-existing target-qualified override must not bypass the trace.
unset "CXX_$target" "CXX_${target//-/_}" HOST_CXX TARGET_CXX
cd "$repo"
CARGO_TARGET_DIR="$work/target-a" LBUG_NATIVE_TRACE="$work/trace-a" \
    scripts/cargo-with-lbug.sh test --locked -p amplihack-cli --features test-support --test lbug_ffi --no-run -vv \
    > "$work/build-a.stdout" 2> "$work/build-a.stderr"
grep -q 'stored local entry' "$work/build-a.stderr"
CARGO_TARGET_DIR="$work/target-b" LBUG_NATIVE_TRACE="$work/trace-b" \
    CMAKE="$work/tripwire/cmake" PATH="$work/tripwire:$PATH" \
    scripts/cargo-with-lbug.sh test --locked -p amplihack-cli --features test-support --test lbug_ffi --no-run -vv \
    > "$work/build-b.stdout" 2> "$work/build-b.stderr"
grep -q 'verified local hit' "$work/build-b.stderr"
ffi=0
core=0
for trace in "$work/trace-b"/invocation.*; do
    compile=0
    kind=other
    while IFS= read -r -d '' arg; do
        case "$arg" in
            -c) compile=1 ;;
            *lbug_rs.cpp | *lbug_arrow.cpp | *cxxbridge*ffi.rs.cc) kind=ffi ;;
            *lbug-src/*.cpp | *lbug-src/*.cc | *lbug-src/*.c) kind=core ;;
        esac
    done < "$trace"
    if [ "$compile" -eq 1 ]; then
        case "$kind" in
            ffi) ffi=$((ffi + 1)) ;;
            core) core=$((core + 1)) ;;
        esac
    fi
done
printf 'clean hit: ffi=%s core=%s\n' "$ffi" "$core" | tee "$work/compiler-summary.txt"
[ "$ffi" -gt 0 ] && [ "$core" -eq 0 ]
count=0
while IFS= read -r -d '' binary; do
    count=$((count + 1))
    "$binary" bounded_native_ --test-threads=1 > "$work/database.stdout" 2> "$work/database.stderr"
done < <(find "$work/target-b/debug/deps" -type f -name 'lbug_ffi-*' -perm -111 -print0)
[ "$count" -eq 1 ]
printf 'Native miss, clean hit and bounded database tests passed; evidence: %s\n' "$work"
