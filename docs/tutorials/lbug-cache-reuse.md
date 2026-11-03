---
title: Reuse lbug Across Two Clean Cargo Targets
description: Build the native binaries twice with one verified static lbug artifact and detect accidental CMake execution.
doc_type: tutorial
last_updated: 2026-11-03
review_schedule: as-needed
---

# Reuse lbug Across Two Clean Cargo Targets

> **[PLANNED - Implementation Pending]** This tutorial describes the intended
> workflow; `scripts/cargo-with-lbug.sh` is not implemented yet.

We will build `amplihack` and `amplihack-hooks` in two fresh Cargo targets using
one verified lbug artifact. Compiler traces distinguish lbug core compilation
from its permitted FFI shim compilation. The second build also uses a CMake
tripwire to detect accidental bundled compilation.

## Contents

- [Prerequisites](#prerequisites)
- [Create owned directories](#1-create-owned-directories)
- [Enable compiler tracing](#2-enable-compiler-tracing)
- [Build the first target](#3-build-the-first-target)
- [Make bundled CMake detectable](#4-make-bundled-cmake-detectable)
- [Build the second target](#5-build-the-second-target)
- [Check compiler traces and binaries](#6-check-compiler-traces-and-binaries)
- [Preserve the exercise](#7-preserve-the-exercise)

## Prerequisites

Use a supported native toolchain, the repository checkout, Bash 3.2 or newer,
Python 3.8 or newer, and network access to an already published compatible artifact.
Keep a C/C++ compiler available: the FFI shim still compiles on a cache hit.
Keep CMake available for ordinary source fallback. See the
[compatibility and configuration reference](../reference/lbug-prebuilt-cache.md).
Use a plain compiler executable in `CXX`, or leave it unset to use `c++`;
this exercise does not support compiler launchers or embedded flags in `CXX`.

Run expensive commands inside an admitted detached session. For a constrained
host, reserve six aggregate CPU threads and 6 GiB, starting with two Cargo jobs
and two native jobs. Record the session handle and fixture directory. Keep Kani
separate from these builds.

## 1. Create owned directories

Run all blocks in the same Bash session, from the repository root:

```bash
set -euo pipefail
lbug_demo="$(mktemp -d "${TMPDIR:-/tmp}/amplihack-lbug-demo.XXXXXX")"
lbug_target="$(rustc -vV | sed -n 's/^host: //p')"
: "${lbug_target:?rustc did not report its native target}"
printf 'lbug_demo=%s\nlbug_target=%s\n' "$lbug_demo" "$lbug_target"
export AMPLIHACK_LBUG_CACHE_DIR="$lbug_demo/cache"
export AMPLIHACK_LBUG_CACHE=auto
export CARGO_BUILD_JOBS=2
export CMAKE_BUILD_PARALLEL_LEVEL=2
unset AMPLIHACK_LBUG_PRODUCER LBUG_SHARED LBUG_LIBRARY_DIR LBUG_INCLUDE_DIR
git rev-parse HEAD > "$lbug_demo/commit.txt"
```

We use one owned artifact cache and two initially absent target directories.
No shared cache is modified. Unsetting `LBUG_SHARED` selects static lbug linking.
Both builds explicitly pass the native host triple as `--target`, overriding
`CARGO_BUILD_TARGET` and Cargo's `build.target`. Even for the host triple, an
explicit target puts release binaries under `<target-dir>/<triple>/release/`,
as described in [Cargo's build options](https://doc.rust-lang.org/cargo/commands/cargo-build.html#compilation-options).

## 2. Enable compiler tracing

Record each C++ compiler invocation in a separate file, preserving its working
directory and arguments without interleaving concurrent calls:

```bash
export LBUG_DEMO_REAL_CXX="$(command -v "${CXX:-c++}")"
: "${LBUG_DEMO_REAL_CXX:?A native C++ compiler is required}"
"$LBUG_DEMO_REAL_CXX" --version > "$lbug_demo/compiler-version.txt"
mkdir "$lbug_demo/compiler" "$lbug_demo/trace-a" "$lbug_demo/trace-b"
export CXX="$lbug_demo/compiler/${LBUG_DEMO_REAL_CXX##*/}"
cat > "$CXX" <<'SH'
#!/usr/bin/env bash
set -eu
trace="$(mktemp "$LBUG_DEMO_TRACE/invocation.XXXXXX")"
printf '%s\0' "$PWD" "$@" > "$trace"
exec "$LBUG_DEMO_REAL_CXX" "$@"
SH
chmod 700 "$CXX"
```

`LBUG_DEMO_REAL_CXX` and `LBUG_DEMO_TRACE` belong only to this tracing fixture.
The wrapper forwards every argument to the real compiler. Each build below
also sets both target-qualified `CXX` forms so an inherited override cannot
bypass the trace. These follow [`cc`'s compiler-variable precedence](https://docs.rs/cc/latest/cc/#external-configuration-via-environment-variables).
Confirm that selection still reports the expected compiler family and major;
an unrecognized tracing wrapper must select source and cannot qualify as a hit.

## 3. Build the first target

```bash
CARGO_TARGET_DIR="$lbug_demo/target-a" LBUG_DEMO_TRACE="$lbug_demo/trace-a" \
env "CXX_$lbug_target=$CXX" "CXX_${lbug_target//-/_}=$CXX" \
scripts/cargo-with-lbug.sh build --target "$lbug_target" \
    --release --locked --bin amplihack --bin amplihack-hooks -vv \
    > "$lbug_demo/target-a.stdout.log" 2> "$lbug_demo/target-a.stderr.log"
```

Expected result: acquisition verifies the archive checksum, immutable release,
manifest, and payload, then reports a managed hit. Cargo compiles the FFI shim
and application without bundled lbug CMake or core compilation. Confirm that
the log reports a verified artifact and retain its exact key.

If this build reports source fallback, the published compatible-artifact
prerequisite was not met. The source build remains valid, but it does not
establish cache reuse. Resolve the diagnostic or follow the
[authorized publication procedure](../operations/lbug-artifact-publication.md)
before continuing this exercise.

## 4. Make bundled CMake detectable

Create a tripwire in the owned fixture:

```bash
mkdir "$lbug_demo/tripwire"
cat > "$lbug_demo/tripwire/cmake" <<'SH'
#!/usr/bin/env bash
printf '%s\n' 'Unexpected CMake invocation during verified lbug hit' >&2
exit 97
SH
chmod 700 "$lbug_demo/tripwire/cmake"
```

The tripwire exits with status 97 if invoked. It leaves the C++ compiler
available for the permitted FFI shim compilation.

## 5. Build the second target

```bash
CARGO_TARGET_DIR="$lbug_demo/target-b" \
LBUG_DEMO_TRACE="$lbug_demo/trace-b" \
PATH="$lbug_demo/tripwire:$PATH" \
CMAKE="$lbug_demo/tripwire/cmake" \
env "CXX_$lbug_target=$CXX" "CXX_${lbug_target//-/_}=$CXX" \
scripts/cargo-with-lbug.sh build --target "$lbug_target" \
    --release --locked --bin amplihack --bin amplihack-hooks -vv \
    > "$lbug_demo/target-b.stdout.log" 2> "$lbug_demo/target-b.stderr.log"
```

Expected result: the second build succeeds, revalidates the same key from the
owned cache, and does not invoke the tripwire. Its fresh Cargo target compiles
the FFI shim. Retain both selection logs and confirm that their verified key
matches. A fast build alone does not establish reuse.

## 6. Check compiler traces and binaries

Summarize actual compilation commands (`-c`), excluding compiler identity and
flag probes. The binding sources and generated `cxxbridge` sources count as
FFI. Other compilation owned by the lbug crate, including bundled core or
third-party sources, rejects the exercise. Other crates' C++ compilation is
reported separately:

```bash
python3 - "$lbug_demo" <<'PY' > "$lbug_demo/compiler-summary.txt"
from pathlib import Path
import os
import sys

root = Path(sys.argv[1])

def lbug_owned(path):
    return any(p == "lbug" or p.startswith("lbug-") for p in path.parts)

for target in ("a", "b"):
    counts = dict(ffi=0, core=0, other=0)
    binding_seen = bridge_seen = False
    for trace in sorted((root / f"trace-{target}").glob("invocation.*")):
        cwd, *args = [os.fsdecode(p) for p in trace.read_bytes().split(b"\0")[:-1]]
        if any(arg.startswith("@") for arg in args):
            sys.exit("Expand compiler response files before qualifying reuse")
        if "-c" not in args:
            continue
        sources = [Path(cwd) / arg for arg in args if not arg.startswith("-")
                   and Path(arg).suffix in (".c", ".cc", ".cpp", ".cxx", ".C", ".c++")]
        if not sources:
            sys.exit(f"Unclassified compilation in {trace}")
        for source in sources:
            if source.name in ("flag_check.c", "flag_check.cpp"):
                continue
            if source.name in ("lbug_rs.cpp", "lbug_arrow.cpp") or (
                "cxxbridge" in source.parts and "lbug" in source.parts
            ):
                kind = "ffi"
                binding_seen |= source.name == "lbug_rs.cpp"
                bridge_seen |= "cxxbridge" in source.parts and "lbug" in source.parts
            elif lbug_owned(Path(cwd)) or lbug_owned(source) or "lbug-src" in source.parts:
                kind = "core"
            else:
                kind = "other"
            counts[kind] += 1
            print(f"target-{target} {kind}: {source}")
    print(f"target-{target}: {counts}", flush=True)
    if not binding_seen or not bridge_seen or counts["core"] != 0:
        sys.exit(f"target-{target} did not demonstrate FFI-only lbug compilation")
PY
cat "$lbug_demo/compiler-summary.txt"
```

Expected result for **each** target: `ffi` is positive and `core` is zero.
The trace must include `src/lbug_rs.cpp` and generated bridge compilation.
The second build must also succeed with the CMake tripwire active. Missing or
unclassifiable traces do not establish reuse; keep them for diagnosis.

Inspect both binaries on the native host, including Cargo's target subdirectory:

```bash
lbug_bin_dir="$lbug_demo/target-b/$lbug_target/release"
for lbug_binary in amplihack amplihack-hooks; do
    test -x "$lbug_bin_dir/$lbug_binary"
    case "$(uname -s)" in
        Darwin)
            otool -L "$lbug_bin_dir/$lbug_binary"
            otool -l "$lbug_bin_dir/$lbug_binary" \
                > "$lbug_demo/$lbug_binary.load-commands.txt"
            ;;
        Linux)
            readelf -d "$lbug_bin_dir/$lbug_binary" \
                > "$lbug_demo/$lbug_binary.dynamic-section.txt"
            ;;
    esac
done
```

Expected result: no dynamic lbug dependency or rpath referring to the cache.
The platform's normal system C++ runtime may remain dynamic.

## 7. Preserve the exercise

Keep the commit, exact artifact key and digest, selection logs, compiler version,
raw traces, summary, native inspection, and session handle in the run's evidence
directory before removing owned targets.
This exercise checks clean-target reuse. Full qualification also exercises
real source fallback, corruption, hooks, installers, relocated binaries, and
lbug persistence through the
[native acceptance procedure](../operations/lbug-artifact-publication.md#retain-native-acceptance-evidence).
Keep point-in-time results in evidence and issue/PR comments.
