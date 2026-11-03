---
title: lbug Cache Scope and Native Qualification
description: Approved local v1 scope, native acceptance requirements, and deferred publication prerequisites.
doc_type: howto
last_updated: 2026-11-03
review_schedule: as-needed
---

# lbug Cache Scope and Native Qualification

## Approved local v1

The owning deputy approved local-only v1 in
[PM328 comment6616249815](https://github.com/mistt-repo/Product-Management/issues/328#issuecomment-6616249815).
This supersedes proposal6615844000 and the earlier pending-approval audit.
The retained decision is `build-support/lbug-cache/approved-v1.md`, with its
SHA-256 and original proposal fingerprints in `approval-v1.json`. Approval of
this reduced scope does not approve the earlier specification documents.

V1 uses the requesting development build's bundled CMake output to populate a
private local manifest cache. It integrates development builds, pre-commit,
pre-push and CI. It does not vendor or patch lbug, publish release assets, change
release/snapshot workflows, or alter npm/uvx installers. Release-profile builds
use bundled source, so upstream's external-mode rpath cannot enter a managed
release. CI checks the actual release binaries' loader metadata.

See the [implemented reference](../reference/lbug-prebuilt-cache.md) and
[two-target native procedure](../tutorials/lbug-cache-reuse.md).

## Retain native acceptance evidence

Run expensive native commands only inside the assigned admitted detached runner.
Record its exact handle and the owned fixture paths. Enforce six aggregate CPU
threads and 6 GiB across nested processes. Cargo/CMake job variables alone do
not establish an aggregate memory bound. Keep Kani separate; do not run builds
on candide or alter installed tools/serialization. Audit test/profile access and
use only owned database and installation fixtures.

Retain evidence for a producing miss, a verified hit in an independent clean
target, zero core C++ and bundled CMake invocations on that hit, separate FFI
compilation, corruption-triggered source compilation, offline/unavailable-cache
fallback, actual hook executions, loader inspection and bounded database
create/query/close/reopen tests. The `lbug_ffi` target includes two bounded native
regressions; those alone do not prove cache selection or native reuse.

The small real Cargo fixture in `tests/test_issue_1917_lbug_cache.py` verifies
warm-target invalidation, source-output preservation, cooperating misses,
interruption and manifest rejection. Its native producer is a substitute. Keep
that evidence separate from actual lbug builds and runtime acceptance.

Required final-head CI, hooks and independent CLEAN reviews remain completion
gates. Record effective review-launch settings rather than inferring them from
requested settings. The macOS `/proc/self/stat` canonical-preflight limitation
remains coordinator-owned; it grants no bypass. Preserve assignment
`CC-AMPLIHACK-LBUG-PREBUILT-1917`, PM328 comment6613591861 and `no_merge=true`.
This implementation step does not authorize a merge.

## Deferred v2 publication

The earlier [artifact-format proposal](../reference/lbug-artifact-format.md) and
`build-support/lbug-cache/v2-contracts/` retain the cross-host design and RED
contracts for a follow-up. They are not installed v1 interfaces or acceptance.

Before implementing publication, the owner must approve the exact follow-up
specification, compatibility/deployment settings and protected producer path.
Producer intent and ordinary authentication do not establish authority. The
previous audit found push access without administration access, an HTTP 404
immutability-settings lookup, and three releases with `immutable=false`; these
observations did not establish current enforcement. No settings were changed.

V2 requires independently verified immutable release evidence bound to the fixed
repository, approved source commit, resolved tag, asset identities/digests and
provenance. A matching archive/checksum pair or `immutable=true` metadata alone
is insufficient. Publication must operate only on an attempt's owned draft,
never overwrite assets or move tags, and revalidate a competing winner. It must
preserve successful requesting source output when packaging/publication fails.
No v1 command publishes or downloads an artifact.
