---
title: Local lbug Cache Reference
description: Verified local static lbug reuse for development builds, hooks, and CI.
doc_type: reference
last_updated: 2026-11-03
review_schedule: as-needed
---

# Local lbug Cache Reference

The spec owner approved **local-only v1** in
[PM328 comment6616249815](https://github.com/mistt-repo/Product-Management/issues/328#issuecomment-6616249815),
superseding the larger publication proposal. The exact decision and its hash are
retained in `build-support/lbug-cache/approved-v1.md` and `approval-v1.json`.
Assignment: `CC-AMPLIHACK-LBUG-PREBUILT-1917`; original assignment: PM328 comment6613591861.

## Cargo wrapper

Run development commands through the checkout's wrapper:

```sh
scripts/cargo-with-lbug.sh build --workspace --locked
scripts/cargo-with-lbug.sh test --workspace --locked
scripts/cargo-with-lbug.sh clippy --locked --all-targets -- -D warnings
```

The wrapper compiles a small dependency-free Rust helper into a private temporary
directory using the already installed compiler. The helper removes that temporary
executable on startup. This requires no application build, Cargo registry access,
or persistent executable cache. Arguments, machine-readable stdout, Cargo status,
target directory and job controls are preserved. SIGINT and SIGTERM reach the
Cargo process group. Diagnostics use stderr and fixed reason text.

## Configuration

| Input | Behavior |
| --- | --- |
| `AMPLIHACK_LBUG_CACHE` | `auto` by default; `source` or an unknown value selects bundled source |
| `AMPLIHACK_LBUG_CACHE_DIR` | Absolute private cache root; unsafe/unusable explicit paths select source |
| `XDG_CACHE_HOME` | Default cache base when defined; otherwise `$HOME/.cache` |
| `LBUG_LIBRARY_DIR`, `LBUG_INCLUDE_DIR`, `LBUG_SHARED` | Every explicit setting, including an empty value, preserves upstream behavior and opts out |
| `--target`, `CARGO_BUILD_TARGET`, simple Cargo `build.target` | Effective target; v1 admits native macOS and GNU/Linux x86-64/aarch64 only |
| `CXX` and target-qualified compiler variables | Resolve the actual C++ compiler and its family/major; unsupported resolution selects source |
| Cargo jobs and target directory | Preserve caller settings |

The wrapper never downloads anything or publishes releases. It sets
`RUSTUP_AUTO_INSTALL=0`, including before bootstrap: Cargo offline mode alone does
not prevent rustup from installing a toolchain. Cargo still needs provisioned
sources and a toolchain for offline builds.

## Selection and fallback

A key is `lbug-0.15.4-<target>-<gcc|clang|appleclang><major>`. The version and
registry checksum come from `Cargo.lock`. A private entry contains `lib/`,
`include/`, and `manifest.sha256`. Its manifest starts with a versioned context
fingerprint and lists every payload file's SHA-256, size and relative path.
It covers exactly the eighteen archives linked by upstream static mode and the
reproducible header closure. The per-target FFI shim is excluded.

Compatibility also checks the full compiler identity, OS/SDK identity, workspace
configuration, and native settings. Machine-specific CPU flags, unsupported Cargo configuration and cross-compilation select
source. A checksum or compatibility rejection warns and selects source for the
whole invocation; rejected entries are not repaired or overwritten. To discard a
bad entry, remove only the exact entry in a cache you own while no builds use it.
Do not clean shared caches as part of tests.

On a miss, the requesting Cargo build compiles upstream bundled lbug normally.
After success, the helper copies those exact native outputs, merges the upstream
include roots, generates `lbug.hpp` from the binding's includes, and compiles the
header without producer include paths. It validates the complete manifest before
atomic installation. A packaging failure retains the successful source result;
there is no second build. Ambiguous native output directories select source
without publishing a guessed payload.

A verified hit sets both external paths and builds the small FFI shim as needed.
Upstream does not track changes to its two path variables. The wrapper therefore
adds a harmless semantic macro to the compiler flags upstream already tracks.
The macro changes when managed selection becomes invalid and stays stable for
valid warm output. Corruption/removal cannot keep linking a previously managed
Cargo result. Plain Cargo also detects the flags disappearing after a wrapped build.

## Cache ownership and concurrency

Cache roots must be private, owned by the current user, and have safe ancestors.
Symlinked roots, payload links, special files, non-ASCII paths, case collisions,
thin archives, extra libraries, missing members and manifest alterations reject
reuse. Staging directories are exclusively created. Installation uses the OS's
atomic no-replacement rename operation; an existing winner is never overwritten.

One wrapper process owns a stable per-key OS lock from preflight until its Cargo
child finishes, including linking and miss packaging. Upstream's build script
never reacquires this lock. Waiters reverify the winner; interrupted staging is
not selectable. Lock files are never unlinked. These are corruption and
cooperating-process controls, not isolation from another process with the same
OS user's write authority. This local manifest is not signed provenance.

## Supported entry points

Development commands, `scripts/dev-space.sh cargo`, actual hook Cargo execution,
CI clippy/nextest/doctests, and the debug standalone probe use the wrapper.
Git-environment scrubbing, checkout-specific targets, TTL ownership, existing
hook checks and CI enforcement remain in place. CI restores a separate local
native cache and saves it only from main. An owned temporary Cargo dispatcher
also routes nested development builds in CI scripts through the verifier; fake
Cargo tools in isolated tests retain precedence. Every use verifies its manifest.

Release/default install/bench profiles and custom profiles use source. Release
and snapshot workflows, npm, uvx, application build scripts, Cargo source patches,
and cxx pins are unchanged. The CI native matrix inspects loader metadata with
`scripts/check-lbug-release-rpath.sh`; release binaries must not reference a local
cache. Development executables can retain upstream's external-mode rpath.

## Verification and deferred work

Run the isolated Rust helper and contract harness, both wrapper/entrypoint Python
suites plus the real Cargo fixture, and npm tests as registered in CI. The small
Cargo fixture proves selection/fingerprint behavior using a substitute native
producer. It does not establish lbug ABI, CMake-free native hits, database runtime
behavior, resource measurements, or passing final-head CI/hooks.

Follow the [two-target tutorial](../tutorials/lbug-cache-reuse.md) for native
acceptance. Signed cross-host release assets, vendor patches, shared publication,
and npm/uvx cache integration are deferred to v2. The
[artifact-format proposal](lbug-artifact-format.md) and
[publication notes](../operations/lbug-artifact-publication.md) describe that
follow-up and are not v1 procedures. Preserve `no_merge=true`; the coordinator
still owns the macOS canonical-preflight handoff and final merge gate.
