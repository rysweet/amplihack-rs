---
title: Prebuilt lbug Cache Reference
description: Build entry points, configuration, identity, and fallback contracts for reusable static lbug builds.
doc_type: reference
last_updated: 2026-11-03
review_schedule: as-needed
---

# Prebuilt lbug Cache Reference

> **[PLANNED - Implementation Pending]** This reference defines the intended
> cache integration; the wrapper and build-support helper are not implemented.

Repository builds reuse verified static LadybugDB (`lbug`) libraries through
`scripts/cargo-with-lbug.sh`. The integration sets `LBUG_LIBRARY_DIR` and
`LBUG_INCLUDE_DIR` for the requesting build. A hit skips bundled lbug CMake and
core C++ compilation. Each Cargo target still compiles its small `cxx` FFI shim.

The cache is optional. Source builds remain available when an artifact, network,
trusted cache, or publication authority is unavailable. Plain Cargo retains its
source-build behavior; use the wrapper for managed reuse.

Contract provenance: [issue #1917](https://github.com/rysweet/amplihack-rs/issues/1917),
assignment `CC-AMPLIHACK-LBUG-PREBUILT-1917`, PM328 comment6613591861.

## Contents

- [Cargo wrapper](#cargo-wrapper)
- [Supported entry points](#supported-entry-points)
- [Artifact identity](#artifact-identity)
- [Configuration](#configuration)
- [Compatibility](#compatibility)
- [Selection and fallback](#selection-and-fallback)
- [Cache ownership and concurrency](#cache-ownership-and-concurrency)
- [Static linking and runtime paths](#static-linking-and-runtime-paths)
- [Diagnostics](#diagnostics)

## Cargo wrapper

From the repository root, replace `cargo` with `scripts/cargo-with-lbug.sh`:

```bash
scripts/cargo-with-lbug.sh build --workspace --locked
scripts/cargo-with-lbug.sh clippy --locked --all-targets -- -D warnings
```

Expected result: Cargo completes normally; cache selection is reported on stderr.
Cargo arguments, their order, `--` boundaries, stdout, and exit status are
preserved. Cancellation reaches child processes and releases invocation locks.
Formatting, metadata queries, and unrelated tool installation do not produce
lbug artifacts.

The wrapper bootstraps only the build-support helper into an owned target.
That helper has no lbug, application-runtime, or third-party Cargo dependencies. Effective offline
settings are resolved before bootstrap. Failed preparation disables managed
caching and passes the original command to Cargo.

## Supported entry points

| Entry point | Cache integration |
| --- | --- |
| Documented application build, check, test, clippy, and nextest commands | `scripts/cargo-with-lbug.sh` with the original Cargo arguments |
| Local source installation | Wrapper `install --path` from the selected checkout for both binaries |
| `scripts/dev-space.sh cargo` | Delegates after selecting its Cargo target and applying its existing ownership and cleanup rules |
| Pre-commit artifact guard and clippy | `scripts/git-hook-cargo.sh` scrubs Git repository variables, selects its per-checkout target, then delegates Cargo execution |
| Pre-push workspace tests | Same hook integration; existing test arguments and enforcement remain in place |
| `ci.yml` application clippy, nextest, doctests, source-install smoke, and native matrix builds | Wrapper runs after target, compiler, and toolchain setup |
| `release.yml` application builds | Wrapper runs with the release target, feature flags, and release-version environment |
| `publish-snapshot.yml` application builds | Wrapper runs with the snapshot target and version environment |
| Containerized application cross-builds | Integration runs inside the container, identifying its compiler; unsupported contexts use source |
| `scripts/probe-no-python.sh` standalone builds | Wrapper builds before the probe restricts PATH; a supplied probe binary is reused |
| npm/npx source fallback | Packaged wrapper builds `amplihack` and `amplihack-hooks` from the distributed workspace |
| uvx source bootstrap | Resolves one source revision, obtains its complete checkout, and invokes that checkout's wrapper for both binaries |

Application release archives are selected independently of lbug prereleases.
Source packages include the wrapper, helper, policies, provenance, required
lockfiles, and complete compatibility source. npm and uvx retain their binary
provisioning checks. Touched shell paths support macOS Bash 3.2.

Formatting, Kani setup/execution, installation of unrelated Cargo tools, and
arbitrary external `cargo install --git` commands are outside managed reuse.
They keep their existing behavior. Existing CI checks and supported release
targets remain enforced even when managed caching selects source.

## Artifact identity

The tag and asset stem use this grammar:

```text
lbug-<locked-version>-<target-triple>-<compiler-family><major>
```

The version comes from the resolved lbug entry in `Cargo.lock`, rather than a
manifest's version range. The target is Cargo's effective target triple, including
`--target`, `CARGO_BUILD_TARGET`, and supported Cargo configuration overrides.
The compiler is resolved inside lbug's Cargo build-script context using the
pinned `cc` behavior. CMake must use that same compiler.

| Component | Example | Identity rule |
| --- | --- | --- |
| Locked version | `0.15.4` | A version change selects a different key |
| Target | `aarch64-apple-darwin` | Host architecture does not override the selected target |
| Compiler family | `gcc`, `clang`, `appleclang` | Apple clang and upstream clang are distinct families |
| Compiler major | `15` | Compiler patch versions do not create another key |

For example, GCC 15 targeting GNU/Linux x86-64 selects
`lbug-0.15.4-x86_64-unknown-linux-gnu-gcc15`. Apple Silicon uses
`aarch64-apple-darwin` and the detected **Apple clang** major. No Apple clang major
is assumed from an Xcode label or a previous machine's report.

Target-specific `CXX` settings, Cargo configuration, compiler launchers, and
cross-compilation overrides participate in resolution. Unreliable identification,
compiler/CMake disagreement, multiple native contexts, multiple targets, and
unsupported configuration interpretation select source. A key alone never
establishes ABI compatibility.

## Configuration

Managed reuse defaults to `auto`; no manual lbug path exports are needed.

| Input | Default | Contract |
| --- | --- | --- |
| `AMPLIHACK_LBUG_CACHE` | `auto` | `auto` selects verified reuse; `source` disables managed acquisition and production. Unknown values warn and select source. |
| `AMPLIHACK_LBUG_CACHE_DIR` | Unset | Absolute path to a dedicated user-owned cache. An explicitly selected unsafe or unusable path selects source. |
| `XDG_CACHE_HOME` | Unset | When absolute, supplies the base for `amplihack/lbug` if no explicit cache directory is set. |
| `HOME` | Inherited | Without either cache override, the cache is `$HOME/.cache/amplihack/lbug`. An unavailable safe home cache selects source. |
| `AMPLIHACK_LBUG_PRODUCER` | Unset | `1` requests production on a confirmed miss. Trusted producer authority is also required; this flag never grants credentials or authority. |
| `LBUG_LIBRARY_DIR`, `LBUG_INCLUDE_DIR` | Unset | Any explicit setting opts out of managed selection. Both, partial, and empty settings retain upstream behavior and upstream errors. |
| `LBUG_SHARED` | Unset | Any defined value, including an empty string, selects upstream shared behavior and opts out of managed static caching. |
| Cargo offline configuration | Inherited | `--offline`, `--frozen`, `CARGO_NET_OFFLINE`, and supported Cargo `net.offline` configuration prohibit helper networking, including bootstrap networking. |
| `CARGO_TARGET_DIR`, `--target-dir` | Cargo defaults | Preserve the caller's application target. Artifact storage is independent of this target. |
| `CARGO_BUILD_JOBS`, `--jobs` | Cargo defaults | Preserve Cargo's resource controls; these are not an aggregate memory limit. |

Explicit cache-directory selection has precedence over XDG and HOME defaults.
An invalid selected location does not redirect writes into a different cache.
Only a trusted, already cached artifact can be used offline. Missing retained
release trust evidence selects source even when an archive is present. Cargo
itself still needs locally available dependencies for an offline source build.

To force a source build:

```bash
AMPLIHACK_LBUG_CACHE=source scripts/cargo-with-lbug.sh build --release --locked
```

Expected result: source selection is reported; a clean lbug target runs CMake.
This setting leaves explicit upstream overrides intact. To request static mode,
**unset** `LBUG_SHARED`; assigning an empty value does not request static mode.

## Compatibility

`build-support/lbug-cache/policies/compatibility-v1.json` is the versioned
allowlist. Selection and production require affirmative agreement on CPU baseline,
target/SDK, C++ deployment baseline, runtime, ABI, features, macros, optimization,
runtime checks, and effective native compiler/linker flags. Custom settings such
as `-march=native`, sanitizers, LTO, or a different C++ runtime select source unless
explicitly covered by policy. Debug and release reuse requires policy coverage
of the native configuration and the per-target shim's macros.

On macOS the C++ deployment baseline is established from the existing release
configuration and native object metadata. Rust's default deployment target does
not establish that baseline. GNU/Linux artifacts retain the supported C++
runtime and release deployment baseline. An absent policy entry selects source.

## Selection and fallback

| Condition | Requesting build behavior |
| --- | --- |
| Verified compatible local hit | Reverify archive, manifest, payload, and trust; supply external static library/include paths; compile only the FFI shim |
| Verified remote artifact | Download archive and mandatory checksum, verify immutable release provenance, validate and atomically install, then consume |
| Confirmed missing key with trusted producer authority | Lock the key, recheck, compile the core once through the requesting build, package those outputs, validate, and publish |
| Missing key without authority | Compile from source; publication credentials are unnecessary |
| Network unavailable, read-only cache, rate limit, authentication failure, or ambiguous service response | Report unavailable service and use source |
| Bad checksum, missing checksum, altered payload, unsafe archive, incompatible provenance, or rejected selection state | Warn, reject the payload, and require source for this entire invocation |
| Packaging or publication fails after a successful source compilation | Reuse that source output; report the separate artifact failure without compiling the core again |
| Source compilation fails | Return the actual build failure |

A rejection is an invocation-wide veto: the same invocation cannot repair or
redownload an artifact and then link it. A tracked build-script input invalidates
previous managed output even in warm Cargo targets. Deleted or unreadable
selection records also invalidate managed reuse. Ordinary valid warm Cargo
output may still be reused after verification.

Successful misses use the requesting build's core output for both linking and
packaging, with at most one source callback in that producing invocation. FFI
shims are per target and are excluded from the artifact. Cooperating producers
sharing one cache serialize attempts and recheck for a successfully published,
verified winner before compiling. A failed or interrupted attempt may require
another compilation in a later invocation. Unpublished local bundles do not
qualify as managed hits. Disconnected producers and consumers using separate
caches do not have global deduplication guarantees.

## Cache ownership and concurrency

The cache separates archives, installed payloads, protected release trust,
selection records, helper builds, staging, and stable per-key OS locks. Managed
directories have restrictive permissions, safe ancestor chains, and current-user
ownership. Rejected paths are never repaired by following symlinks.

Installation uses exclusive staging and an atomic operation that cannot replace
an existing winner. Waiters acquire the same key lock and reverify the winner.
Interrupted staging is never selectable. Active lock files are not unlinked, and
installed payloads are never modified in place. Protection continues through
compilation and linking.

Hook Cargo targets remain checkout-specific. Their TTL cleanup does not reclaim
the lbug artifact cache. Test fixtures use owned cache roots and do not corrupt,
remove, or clean shared caches. Same-user filesystem permissions do not isolate
mutually untrusted processes; privileged producers use a separate trust domain.

## Static linking and runtime paths

With `LBUG_SHARED` unset, lbug and its third-party archives link statically.
System C++ runtime dependencies retain the platform's existing behavior.
The tracked compatibility patch omits upstream external static-mode rpath
emission, including an rpath to a developer's cache. It preserves explicit
library overrides and shared-mode behavior. `crates/amplihack-cli/build.rs`
remains the reserved no-op.

Released binaries run after relocation with the build cache unavailable and
loader overrides removed. Verification includes `otool` on macOS or `readelf`
on GNU/Linux and a real lbug-backed operation; static link flags alone do not
establish runtime independence.

## Diagnostics

Cache diagnostics go to stderr and include severity, a stable reason code,
selection stage, a sanitized key when known, and the selected action. Corruption
always produces a visible warning explaining that the build uses source.
Diagnostics distinguish core compilation, FFI shim compilation, acquisition,
packaging, publication, and source-build failure. They never include tokens,
authorization headers, signed URLs, environment dumps, or arbitrary arguments.

For the payload and internal build-support contracts, see
[lbug artifact format](lbug-artifact-format.md). For hands-on usage, follow
[reuse lbug across two Cargo targets](../tutorials/lbug-cache-reuse.md). Trusted
producers use [publish a lbug artifact](../operations/lbug-artifact-publication.md).
