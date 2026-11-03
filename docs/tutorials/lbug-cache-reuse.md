---
title: Reuse lbug Across Two Clean Cargo Targets
description: Qualify local development cache reuse with actual native builds.
doc_type: tutorial
last_updated: 2026-11-03
review_schedule: as-needed
---

# Reuse lbug Across Two Clean Cargo Targets

V1 caches local development outputs. It needs an installed Rust toolchain,
C/C++ compiler and CMake, with Cargo dependencies provisioned for offline work.
No published artifact, credentials or Python runtime is required.

Run the following inside the assigned admitted detached runner, with aggregate
resource enforcement and owned fixtures as described in the
[native qualification notes](../operations/lbug-artifact-publication.md#retain-native-acceptance-evidence).
Record the runner handle. The script reports its owned fixture directory and
keeps it for inspection:

```sh
scripts/verify-lbug-native.sh
```

The first clean target must report a local miss followed by a stored entry.
It compiles actual bundled lbug source and packages that output. The second
clean target must report a verified hit and succeed with a fail-on-use CMake
tripwire. Compiler traces must show separate FFI compilation and zero bundled
core compilations on the second build. The script then executes only the two
bounded persistence/error-recovery regressions in the second test binary.

A failed packaging step or source fallback leaves a valid source build but does
not qualify as a cache hit. Inspect the logs; do not claim reuse from elapsed
time alone. Keep the raw traces, manifest, compiler identity, commit and logs
before removing any owned targets.

To qualify corruption and offline fallback, use a separate copy of the owned
fixture cache, change one archive byte, and repeat a clean debug build. It must
warn, select actual bundled source and succeed. Repeat offline with no cache
entry and a provisioned toolchain/dependency cache. The wrapper disables rustup
automatic installation; Cargo offline alone is insufficient.

For release linkage, build using the source release profile and inspect both
actual binaries:

```sh
cargo build --release --locked --bin amplihack --bin amplihack-hooks
scripts/check-lbug-release-rpath.sh target/release/amplihack target/release/amplihack-hooks
```

The check reads `otool -l` on macOS or `readelf -d` on GNU/Linux and rejects local
cache loader paths. These commands are separate from development cache reuse;
release and installation builds never receive managed cache paths. Development
binaries may retain upstream's external-mode rpath. Relocated release runtime,
actual hooks, final-head CI and independent review still require retained
acceptance evidence.
