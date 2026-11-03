---
title: lbug Artifact Format and Build-Support Contracts
description: Archive inventory, integrity limits, provenance, and internal contracts for prebuilt lbug artifacts.
doc_type: reference
last_updated: 2026-11-03
review_schedule: as-needed
---

# lbug Artifact Format and Build-Support Contracts

> **[DEFERRED v2 PROPOSAL]** These cross-host artifact and build-support
> contracts were not approved for v1. The spec owner approved the
> [local-only cache](lbug-prebuilt-cache.md) in PM328 comment6616249815.
> Paths and commands below are proposed follow-up work, not installed interfaces.

This reference defines the reusable static payload consumed by the
[lbug cache integration](lbug-prebuilt-cache.md). The build-support library and
executable are internal to repository builds; they add no application API,
database schema, or HTTP service.

## Contents

- [Release assets](#release-assets)
- [Archive layout](#archive-layout)
- [Manifest and provenance](#manifest-and-provenance)
- [Headers and licenses](#headers-and-licenses)
- [Integrity and extraction](#integrity-and-extraction)
- [Selection records](#selection-records)
- [Internal contracts](#internal-contracts)

## Release assets

The fixed repository is `rysweet/amplihack-rs`. For the key defined in
[artifact identity](lbug-prebuilt-cache.md#artifact-identity), the release tag
is the key and the two required assets are:

```text
<key>.tar.gz
<key>.tar.gz.sha256
```

The checksum file contains exactly one ASCII line: 64 lowercase hexadecimal
SHA-256 characters, two spaces, the exact archive basename, and a terminating
newline. Extra lines, paths, mismatched filenames, malformed digests, and a
missing checksum reject the artifact.

Asset URLs use
`https://github.com/rysweet/amplihack-rs/releases/download/<key>/<key>.tar.gz`
and the same URL with `.sha256` appended. The published release is immutable,
public, a prerelease, and excluded from latest application-release selection.
Release-note text is mutable and is never a digest trust anchor. GitHub's
[immutable release contract](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)
protects published assets and the tag and supplies release attestation evidence.

## Archive layout

Archive paths are relative to the payload root, with no enclosing key directory:

| Path | Contents |
| --- | --- |
| `manifest.json` | Versioned artifact manifest |
| `lib/` | Exactly the 18 regular static archives below |
| `include/lbug.hpp` | Deterministically generated public entry header |
| `include/` | Complete binding and transitive include closure, including generated configuration headers |
| `licenses/` | lbug and bundled third-party licenses and required notices |
| `provenance/` | Source identity, compatibility policy, patch, header-recipe, and approved producer records |

The complete lbug 0.15.4 static inventory is:

```text
lib/liblbug.a
lib/libutf8proc.a
lib/libantlr4_cypher.a
lib/libantlr4_runtime.a
lib/libre2.a
lib/libfastpfor.a
lib/libparquet.a
lib/libthrift.a
lib/libsnappy.a
lib/libzstd.a
lib/libminiz.a
lib/libmbedtls.a
lib/libbrotlidec.a
lib/libbrotlicommon.a
lib/liblz4.a
lib/libroaring_bitmap.a
lib/libsimsimd.a
lib/libyyjson.a
```

This inventory follows the pinned crate's static link directives, rather than
every archive found in a Cargo output directory. Thin archives, dynamic lbug
libraries, per-target FFI shim archives, databases, credentials, unrelated build
outputs, and unapproved files are excluded. Object metadata must match the target
architecture, deployment baseline, runtime, and compatibility policy.

## Manifest and provenance

Schema version 1 is defined by
`build-support/lbug-cache/schemas/artifact-v1.json`. Its required information is:

| Record | Required information |
| --- | --- |
| Schema and identity | Schema version, exact key, locked lbug version, target, compiler family and major |
| Compiler | Effective compiler identity and full version; agreement with the actual CMake compiler |
| Source | Registry archive SHA-256, upstream source provenance, approved repository commit, integration patch hash, and upstream-file inventory hash |
| Header recipe | Versioned generator identity, pinned inputs and their hashes, and include-root merge policy |
| Compatibility | Policy version/hash and the effective CPU, SDK, deployment, runtime, ABI, feature, macro, and native flag settings |
| Payload inventory | Every payload file's relative path, role, byte length, and SHA-256, excluding the manifest itself |
| Producer | Trusted build provenance bound to the approved clean commit and compatibility inputs |

The externally verified archive covers `manifest.json`; it does not hash itself.
The manifest describes every other file exactly once. Duplicate JSON keys,
duplicate paths, extra or missing files, unsupported required schema features,
and identity disagreements reject the bundle.

The pinned upstream `lbug-0.15.4.crate` digest is:

```text
7ff191ec195fb257dd7ca2adb76afd9a4b9a9f60fa27f1d87e5a958849fe9e6f
```

Repository provenance lives under `build-support/lbug-cache/provenance/`;
`upstream-files.json` records the complete import. The reviewed compatibility
delta is recorded in `build-support/lbug-cache/patches/lbug-0.15.4.patch`.
Original source bytes and licenses are preserved apart from that tracked delta.
The root Cargo patch selects the compatibility copy, following
[Cargo's patch mechanism](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html#the-patch-section).
Existing `cxx` and `cxx-build` pins remain aligned.

Consumers derive expected source, patch, header-recipe, and policy hashes from
their reviewed checkout, independently of the archive. Matching an attacker-
supplied archive to its matching checksum is insufficient. Trust evidence binds
the repository identity, release ID, resolved tag commit, asset IDs/digests, and
expected provenance to the immutable release. Offline use requires retained,
protected evidence of that verification.

The key does not include patch or policy hashes. If those inputs change under
an existing immutable key, an incompatible artifact is rejected and the build
uses source. Publication never overwrites that key to update its contents.

## Headers and licenses

Header generation starts from the pinned source and binding include list,
including the optional Arrow binding when enabled by policy. It is reproducible
without harvesting an ordinary Cargo target as the only source of headers.

The closure includes public/internal headers and required generated headers from
the lbug source and native build include roots, plus `nlohmann_json`, `fastpfor`,
and `alp/include` where required. Merging roots at one relative include path is
permitted only for byte-identical files. Conflicting files reject packaging.
Generated values are tied to the manifest's compatibility settings.

A source-independent header compilation and bounded native create/query/reopen
smoke operation precede publication. These checks resolve includes from the
bundle and approved system toolchain without producer source-directory paths.
License collection covers lbug and every bundled third-party component. Payload
inspection rejects secrets and unapproved embedded producer paths.

## Integrity and extraction

Payload validation checks the archive against its checksum before extraction,
then checks the schema, expected provenance, inventory, headers, licenses,
object compatibility, and every extracted file. These checks and the extraction
limits below apply to both locally produced bundles and published artifacts.

Before publication, a producer validates its generated archive and checksum
against the approved source output and reviewed checkout, then runs the header
and native smoke checks. No immutable-release attestation exists for an
unpublished bundle. A valid local payload is eligible for upload, but does not
yet qualify as a managed cache hit. The requesting build can use its successful
`NativeBuildOutput` directly, including after packaging or publication failure.

After publication, the producer independently verifies the public immutable
release, resolved tag commit, and both uploaded assets before recording protected
release trust evidence. A consumer requires that evidence as well as payload
validation: on acquisition and each reuse, it verifies the archive and sidecar
against the release's anchored asset evidence and rechecks the digest, manifest,
and extracted files before linking. Offline reuse requires retained protected
release evidence. A locally generated checksum alone cannot supply release trust.

| Limit | Maximum |
| --- | --- |
| Compressed archive | 1 GiB |
| Expanded payload | 4 GiB |
| Individual file | 2 GiB |
| Archive members | 50,000 |
| Path length | 1,024 bytes |
| Path components | 32 |
| Manifest | 16 MiB |
| API metadata response | 2 MiB |

Streamed byte counts and checked arithmetic enforce these limits during
processing. Untrusted header sizes alone cannot establish that a limit is met.

Extraction resolves effective names after archive extensions and rejects
absolute paths, traversal, duplicate entries, case/Unicode filesystem collisions,
links, sparse files, devices, sockets, FIFOs, and other special members. Paths,
keys, and metadata reject control characters and excessive lengths. Ownership,
privileged modes, ACLs, and extended attributes from the archive are ignored.

Descriptor-relative operations that do not follow links create files in an
exclusive staging directory. Installation cannot replace an existing payload;
an existing winner is independently reverified. Failed or interrupted staging
never becomes selectable. Integrity failure warns and triggers the invocation-
wide source veto described in [selection and fallback](lbug-prebuilt-cache.md#selection-and-fallback).

## Selection records

`build-support/lbug-cache/schemas/selection-v1.json` defines protected, versioned
selection records. A record binds one invocation/native context to its resolved
request, verified payload and trust evidence, effective configuration, and
selection or source-veto state. Managed lbug paths are accepted only through a
record validated against the current request.

Before Cargo runs, preflight revalidates the selected artifact even if Cargo's
target is warm. A build-script-tracked input changes when selection is rejected,
deleted, unreadable, or altered, preventing Cargo from reusing an earlier managed
link result without running the required source path. Records and payload paths
cannot be substituted with another key's otherwise valid contents.

## Internal contracts

The following are semantic build-support interfaces. They do not expose stable
application Rust function signatures.

| Contract | Inputs and result | Invariant |
| --- | --- | --- |
| Build request | Locked source, effective Cargo target/compiler, compatibility settings, offline state, invocation identity, and explicit upstream overrides | Identity comes from the requesting native context; ambiguity selects source |
| Prepared bundle | Locally generated archive and checksum validated against source output, expected provenance, and payload rules | Eligible for upload; not a managed cache hit before public immutable-release verification |
| Verified payload | Validated archive, inventory, extracted roots, expected provenance, and verified public release trust | Managed library/include paths are unavailable before payload and release verification complete |
| `NativeBuildOutput` | Source callback result containing native archives, include roots, and actual compiler/configuration metadata | Core compilation returns before link directives are emitted; the same output supplies packaging and the requester |
| Selection | Verified payload, explicit upstream mode, or source with a reason | Rejection vetoes managed artifacts throughout the invocation |
| Producer | Request, independently trusted authority, key lock, and source callback | Calls the core source callback at most once; later packaging/publication failure preserves its successful output |
| Publication | Prepared bundle, checksum, approved commit, and owned draft attempt; returns public immutable-release evidence after verification | Modifies only its owned draft; consumes a verified existing winner without replacement; local payload validity never substitutes for release trust |
| Diagnostic | Severity, stable reason code, stage, safe identity fields, and action | Stderr only; bounded sanitized fields; no secret-bearing values |

`AMPLIHACK_LBUG_PRODUCER=1` expresses intent only. Producer authority is supplied
by trusted execution tied to reviewed source, and cannot be manufactured from
an environment flag or an untrusted selection record. Untrusted PR execution
does not receive publication credentials or writable access to producer helpers,
targets, or caches.
