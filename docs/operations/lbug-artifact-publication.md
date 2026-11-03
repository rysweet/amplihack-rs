---
title: Publish and Verify a lbug Artifact
description: Produce a native static lbug bundle through a trusted requesting build and verify its immutable release.
doc_type: howto
last_updated: 2026-11-03
review_schedule: as-needed
---

# Publish and Verify a lbug Artifact

> **[PLANNED - Implementation Pending]** This procedure describes the intended
> producer and verification behavior; its lbug helper scripts are not implemented.

Use this procedure to populate a missing artifact key through the same source
compilation that serves an application build. Publication runs through the
[shared Cargo integration](../reference/lbug-prebuilt-cache.md), preserving
working source output if packaging or release operations fail.

## Contents

- [Issue #1917 readiness audit](#issue-1917-readiness-audit)
- [Prepare an authorized native producer](#prepare-an-authorized-native-producer)
- [Build and publish the missing key](#build-and-publish-the-missing-key)
- [Verify an independent download](#verify-an-independent-download)
- [Handle publication failures and races](#handle-publication-failures-and-races)
- [Retain native acceptance evidence](#retain-native-acceptance-evidence)

## Issue #1917 readiness audit

The implementation step inspected baseline
`c89e50c3887563c8b0eac4283c25ac255631c3f3` on 3 November 2026, under assignment
`CC-AMPLIHACK-LBUG-PREBUILT-1917`,
[PM328 comment6613591861](https://github.com/mistt-repo/Product-Management/issues/328#issuecomment-6613591861).
Deputy specification approval remains outstanding; the concrete interface and
specification hashes were submitted in
[comment6615844000](https://github.com/mistt-repo/Product-Management/issues/328#issuecomment-6615844000).
Independent specification review does not substitute for that approval.

- The downloaded registry archive matches the pinned SHA-256
  `7ff191ec195fb257dd7ca2adb76afd9a4b9a9f60fa27f1d87e5a958849fe9e6f`.
  Its inventory contains 2,707 members and 38,125,469 uncompressed bytes.
- This Mac's `clang++ --version` reports Apple clang 21.0.0. The proposed native
  key is `lbug-0.15.4-aarch64-apple-darwin-appleclang21`; deployment, runtime,
  and ABI compatibility still require native evidence and approval.
- The authenticated repository permission query reports push access without
  administration access. The immutability-settings query returns HTTP 404.
  The three inspected releases (`v1.2.94`, `v1.2.93`, and
  `snapshot-e8ac13159aed70f0ad69394ea99fd64e72138e25`) report `immutable=false`.
  These observations leave current immutable-release enforcement unverified;
  they do not establish that it is disabled. Repository settings remain unchanged.
- The protected producer execution path and approved-commit evidence need an
  owner decision. An environment flag and successful GitHub authentication alone
  cannot grant the required producer authority.
- The existing macOS canonical preflight failure at `/proc/self/stat` remains
  a coordinator-owned handoff constraint. This work grants no gate bypass or
  manual merge authority.

The source audit and unresolved publication prerequisite are recorded on
[issue #1917](https://github.com/rysweet/amplihack-rs/issues/1917#issuecomment-6615886459).
No native build or release publication was performed for this audit. The new
contracts remain RED; no cache-hit, source-fallback, linkage, installation, or
runtime acceptance result is claimed.

## Prepare an authorized native producer

1. Use a clean, approved source commit and the reviewed provenance and
   compatibility policy. Verify the complete pinned source import using
   `scripts/check-lbug-vendor.sh`. Keep `cxx`/`cxx-build` pins unchanged.
2. Enter the trusted producer execution path with release-asset authority for
   `rysweet/amplihack-rs`. Use normal scoped GitHub authentication. The producer
   verifies authority independently of `AMPLIHACK_LBUG_PRODUCER`; setting that
   flag in an ordinary consumer shell grants no authority.
3. Confirm that GitHub immutable releases are enforced. Missing enforcement
   prevents artifact publication; the helper does not change repository settings.
4. Reserve owned cache, Cargo target, installation, and evidence directories.
   Keep privileged producer outputs separate from untrusted PR execution.
5. Admit the complete process tree through the host's detached resource runner.
   For constrained qualification, enforce at most six aggregate CPU threads and
   6 GiB; start with two Cargo jobs and two native jobs. Job environment variables
   alone do not enforce aggregate memory or nested-process limits. Run Kani
   separately.

On Apple Silicon, use the native `aarch64-apple-darwin` toolchain on the Mac.
Verify the effective Apple clang family and major and the existing C++ deployment
baseline before production. Do not substitute upstream clang numbering or
infer the deployment baseline from Rust defaults.

## Build and publish the missing key

Run the following from the repository root **inside the admitted detached
execution**. `lbug_work` creates a dedicated fixture; record its value and the
runner's exact session handle before starting the build.

```bash
lbug_work="$(mktemp -d "${TMPDIR:-/tmp}/amplihack-lbug-producer.XXXXXX")"
printf 'lbug_work=%s\n' "$lbug_work"
git rev-parse HEAD > "$lbug_work/commit.txt"
export AMPLIHACK_LBUG_CACHE_DIR="$lbug_work/cache"
export CARGO_TARGET_DIR="$lbug_work/target"
export CARGO_BUILD_JOBS=2
export CMAKE_BUILD_PARALLEL_LEVEL=2
unset LBUG_SHARED LBUG_LIBRARY_DIR LBUG_INCLUDE_DIR
AMPLIHACK_LBUG_PRODUCER=1 scripts/cargo-with-lbug.sh build \
    --release --locked --target aarch64-apple-darwin \
    --bin amplihack --bin amplihack-hooks \
    > "$lbug_work/build.stdout.log" 2> "$lbug_work/build.stderr.log"
```

Expected result on a confirmed missing key: the helper acquires the key lock,
rechecks local and remote state, and invokes the requesting build's core source
callback once. Those outputs both link the application and supply the reusable
bundle. Header generation, payload verification, and bounded native smoke checks
complete before publication. This establishes payload validity; immutable-release
trust is established only after publication. The FFI shim compiles separately
for this target.

The producer creates an owned draft at the approved commit, verifies the actual
tag target, uploads the archive and checksum, and compares uploaded bytes and
digests with the prepared bundle. It publishes the draft as a public immutable
prerelease with `make_latest` set to the **string** `"false"`, then independently
verifies the public release and both assets against immutable-release evidence.
Only after that verification can the bundle become a managed cache hit with
protected release trust evidence. The source output remains usable if publication
or its verification fails. See the [validation stages](../reference/lbug-artifact-format.md#integrity-and-extraction).

An already published compatible key is a verified hit, not evidence of a new
producing miss. Never delete a release or alter its key to force the miss case.

## Verify an independent download

Use a fresh download directory. Enter the exact key reported by the producer;
do not guess the Apple clang major. Run the following as one Bash block. It
performs read-only release operations and exits the subshell on the first failed
release, download, asset, checksum, or listing command:

```bash
(
    set -euo pipefail
    IFS= read -r -p 'Artifact key reported by the producer: ' lbug_key
    : "${lbug_key:?An artifact key is required}"
    lbug_download="$(mktemp -d "${TMPDIR:-/tmp}/amplihack-lbug-download.XXXXXX")"
    gh release verify "$lbug_key" --repo rysweet/amplihack-rs
    gh release download "$lbug_key" --repo rysweet/amplihack-rs \
        --pattern "$lbug_key.tar.gz" --pattern "$lbug_key.tar.gz.sha256" \
        --dir "$lbug_download"
    gh release verify-asset "$lbug_key" "$lbug_download/$lbug_key.tar.gz" \
        --repo rysweet/amplihack-rs
    gh release verify-asset "$lbug_key" "$lbug_download/$lbug_key.tar.gz.sha256" \
        --repo rysweet/amplihack-rs
    cd "$lbug_download"
    shasum -a 256 --check "$lbug_key.tar.gz.sha256"
    tar -tzf "$lbug_key.tar.gz" > payload-members.txt
    printf 'Verified download: %s\n' "$lbug_download"
)
```

Expected result: release and asset verification succeed and the checksum command
prints the archive filename followed by `OK`. Retain the downloaded sidecar as
the actual artifact digest; never replace it with an illustrative hash.
The [GitHub release verification commands](https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/secure-your-dependencies/verify-release-integrity)
verify immutable release evidence and asset identity.

Compare the member list with the
[18-archive inventory and header contract](../reference/lbug-artifact-format.md#archive-layout).
Listing an archive does not validate its extraction safety. A subsequent wrapper
build performs strict schema, provenance, archive, header, and extracted-payload
validation before supplying linker paths.

## Handle publication failures and races

Only a confirmed missing key enables production. Authentication errors, rate
limits, incomplete metadata, and ambiguous responses mean unavailable service.
Use the successful source output and retain a separate publication failure record.
A successful application build alone does not prove publication succeeded.

If another producer wins, verify and consume that immutable release without
replacing assets or moving its tag. Pending drafts, upload collisions, and
uncertain publish responses are reconciled within bounded retries. Never edit
another attempt's draft or repair an invalid public release. An incompatible
winner selects source for that invocation.

Networking uses TLS verification and the fixed repository, validates API/upload
origins, and never forwards credentials across origins. Limits are five
redirects, two transient retries, 10-second connection establishment,
120-second metadata requests, 300-second transfers, and a 600-second overall
acquisition or publication deadline. Offline mode forbids helper network calls.
Publication failure does not trigger permission escalation.

## Retain native acceptance evidence

Use `scripts/verify-lbug-native.sh` as the native acceptance entry point. Keep
each scenario's cache/target fixtures separate. Audit the selected executable
and tests for real-profile access first; a temporary HOME provides hygiene,
not process isolation. Database fixtures use bounded settings, such as a
64-MiB pool and one worker. CLI graph tests set `AMPLIHACK_GRAPH_DB_PATH` to an
owned fixture and use the repository's isolated-command helper.

Retain evidence for each condition independently:

| Scenario | Required evidence |
| --- | --- |
| Producing miss | One core compilation supplies the requesting build and published bundle; independent asset download and digest verification |
| Two clean targets | Sequential builds select the same key; native invocation logs distinguish the FFI shim and prove no bundled CMake/core compilation on the second |
| Detectable CMake hit test | The clean hit succeeds with bundled CMake configured to fail if invoked |
| Corruption | An owned archive copy has a bad digest, visibly warns, is never linked, and the actual source build succeeds |
| Additional integrity rejection | Missing checksum, altered extracted payload, forged provenance, and rejected warm-target selection require source |
| Unavailable cache | Offline or inaccessible-cache source build succeeds without publication credentials |
| Producer and installation races | Lock waiters recheck; interrupted staging remains unavailable; an existing publication is never overwritten |
| Hooks and CI | Actual pre-commit and pre-push executions use the integration and retain their checks; applicable CI passes on the final head |
| Distributed installation | Build from unpacked npm content and the selected uvx checkout; provision and execute both binaries |
| Release runtime | With `LBUG_SHARED` unset, inspect `otool`/`readelf` output, relocate to an owned installation, remove loader overrides, make its owned cache unavailable, and execute CLI plus lbug create/query/reopen operations |

Record exact commands, detached handles, final commit, effective launch settings,
compiler/target identity, policy/provenance hashes, asset URLs and SHA-256, native
logs, resource measurements, and results outside `docs/`, in the run's evidence
directory and issue/PR handoff. Retain evidence before deleting owned targets;
never clean shared caches or another worker's files.

Return platform adaptations to source documentation and put run-specific hashes,
artifact URLs, results, and evidence links in the PM328 handoff; notify bb through
the assigned coordination channel. Native macOS qualification does not satisfy
the Linux canonical merge preflight, which reads `/proc/self/stat`. Retain that
limitation in the handoff for the enclosing workflow's canonical gate. It grants
no permission to bypass hooks or gates. A build phase using
`no_merge=true` ends with an open, merge-ready PR; its enclosing workflow owns
subsequent merging. Coordinator-owned host serialization is removed only by
that coordinator after shipment.
