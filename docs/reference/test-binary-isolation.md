---
title: Test binary isolation
description: Proposed discovery, staging, and footprint contracts for fake-home CLI tests.
status: planned
type: reference
issue: 2182
---

# Test binary isolation [PLANNED - Implementation Pending]

This reference specifies the proposed behavior for [issue #2182](https://github.com/rysweet/amplihack-rs/issues/2182). It replaces the planned `Specs/` document for this feature. All contracts below describe implementation requirements, not verified behavior.

The working implementation now links executable files larger than 1 MiB at final release staging under `cfg(any(test, feature = "test-support"))`. It tries a hardlink, then a symlink, and never chmods the linked source. Smaller fixtures retain existing copy behavior. Test builds use a private inert resolver before sibling or PATH discovery. Production installation remains unchanged; its optimization is deferred to #2147.

The artifact registry records canonical paths, filesystem identities on Unix, size, and modification time. The footprint verifier inspects publication entries and retained or unpublished release bins, resolves symlinks, and rejects oversized files without registered provenance. Exactly 1 MiB is accepted. Existing shell fixtures are approximately 1 KiB; the regression fixtures exercise both sides of the limit without allocating real resolver binaries.

The subprocess helper now requests a dedicated `test-support` build and rejects unverified executable overrides. The shared command fixture explicitly sets application roots and PATH. These changes are partial: caller migration, real-resolver fixture selection, explicit cleanup-time verification, and behavioral validation of executable overrides still need completion.

### Validation recorded during implementation

- Baseline revision: `09d39e883f45fbfb541d8aa20aa4dbd5df77a33b`.
- Fourteen new unit contracts passed; two subprocess command-configuration contracts passed; ten existing binary-deployment tests and 75 release-layout tests passed.
- The broader installation run finished with 473 passed, 144 failed, and one ignored. A focused source-gate rerun traced failures to `src/test_support.rs:48`, which attempts to create temporary directories under read-only `/var/tmp`. Its output also includes attempts to update a global npm installation rejected by the read-only filesystem. This run does not establish complete fixture isolation or a passing installation suite.
- A rerun with `TMPDIR=/tmp AMPLIHACK_SKIP_MMDC=1` finished with 477 passed, 140 failed, and one ignored. Remaining failures include `/var/tmp` creation, rejection of the sandbox's root-directory permissions by status-line ownership checks, and a native Codex inventory probe. Full output was retained in `/tmp/issue2182-install-isolated.log`; these failures have not been classified against the baseline.
- The pinned Rust 1.97 formatting shim could not write to the read-only Rustup directory. Formatting succeeded using the installed 1.99.1 toolchain explicitly.
- `scripts/measure_issue_2182.py` provides Linux subreaper/zombie I/O accounting and preserves full logs, but has not yet been validated with comparable baseline and fixed runs. No before/after write totals or full-suite success are claimed.

No PR or merge evidence has been recorded. The planned contracts below remain the completion criteria.

Remove the planned labels only after implementation and verification, with links to the caller inventory and measurement evidence described below.

Fake-home tests use private resolver stubs or verified links to built artifacts. Installation and indirect self-heal preserve these links through final release staging, preventing repeated copies of large executables.

This policy covers direct library installs, subprocess installs, profile operations, recipes, launches, executable overrides, and nested builds. It protects the CLI and other staged executables as well as `amplihack-asset-resolver`.

## Contents

- [Discovery and staging](#proposed-discovery-and-staging)
- [Fixture configuration](#proposed-fixture-configuration)
- [Internal support contracts](#proposed-internal-support-contracts)
- [Footprint limit](#proposed-footprint-limit)
- [Regression coverage](#proposed-regression-coverage)
- [Write measurement contract](#proposed-write-measurement-contract)
- [Production scope](#proposed-production-scope)

## Proposed discovery and staging

The binary policy is compiled under `cfg(any(test, feature = "test-support"))`. Normal production builds retain their existing discovery and copy behavior. There is no public CLI switch or production environment-variable bypass for this policy.

When resolver behavior is irrelevant, a fixture supplies a small executable stub. The policy controls both discovery next to `current_exe()` and discovery through PATH. Missing private resolver configuration fails explicitly instead of searching the host. Adding a PATH stub alone is insufficient because adjacent-executable discovery can take precedence.

When a test exercises real executable behavior, the fixture registers the built artifact and links it into the fake home. Final staging attempts a hardlink first, then a symlink if hardlinking fails, including across filesystems. If both fail, staging returns an error; it never copies the artifact. This rule applies inside release directories as well as at published `.local/bin` paths.

Subprocess tests use a protected CLI built once with `test-support` and the features their tests require, in dedicated test output. Fixture construction does not change shared Cargo output directories. Private executable directories, resolver fixtures, and immutable artifact sources remain alive until children finish and footprint verification succeeds. Protection remains effective when `current_exe()` resolves a relocated symlink back to build output.

## Proposed fixture configuration

| Setting | Fixture contract |
| --- | --- |
| `HOME` | Private temporary home owned by the fixture. |
| `AMPLIHACK_HOME` | Private application root, wherever the tested path uses it. |
| `CLAUDE_CONFIG_DIR` | Private Claude configuration directory. |
| `AMPLIHACK_CLAUDE_PROFILE_ROOT` | Private profile root, wherever profiles are used. |
| `PATH` | Controlled resolver discovery and explicitly required tools. |
| `AMPLIHACK_TEST_BINARY` | Optional executable override validated for protected discovery and staging before reuse. |

Fixtures remove inherited installation overrides that could redirect writes outside their roots. Toolchain access uses explicit Cargo and Rustup paths without restoring uncontrolled resolver discovery. Tests never read or modify real user application or profile roots.

In-process environment changes are scoped under the existing environment mutex and restore prior values on exit. A test-only unit configuration does not protect a separate executable: subprocesses must run the feature-enabled protected CLI. Direct integration-library callers also use the feature-enabled policy.

An override must identify an executable file and pass behavioral checks for discovery and staging. An ordinary production CLI does not qualify merely because it exists or reports the expected version. Invalid overrides fail with a diagnostic instead of falling back to an unprotected binary.

## Proposed internal support contracts

The following components and responsibilities are proposed test-support contracts, not available public application APIs. The existing `amplihack_bin.rs` helper requires changes to meet its listed contract.

| Component | Contract |
| --- | --- |
| `commands::install::test_support` | Owns test discovery and staging policy used at `stage_binary()`. |
| `test_support::artifacts` | Validates executable sources, registers canonical paths and immutable identities, and creates verified links. |
| `test_support::footprint` | Inspects published paths and release binaries against registered artifacts. |
| `tests/support/amplihack_bin.rs` | Builds and reuses the protected subprocess executable. |
| `tests/support/isolated_amplihack.rs` | Owns private application roots, discovery configuration, artifact lifetimes, and verification before cleanup. |

Artifact registration validates an existing executable source outside release trees. On Unix, identity uses device and inode. Other platforms use supported identity verification or reject oversized artifacts when verification is unavailable.

Link creation validates destination confinement before replacing a directory entry. It never truncates through an existing link and never changes linked destination permissions. Sources must already have executable permissions. An injectable filesystem-operation seam supports deterministic hardlink failure and total link failure tests without timers.

Fixtures perform explicit, fallible footprint verification before normal cleanup. Cleanup during unwinding preserves the original failure and emits safe diagnostics without a second panic.

## Proposed footprint limit

The independent-file limit is **1 MiB (1,048,576 bytes)** per staged binary. A file exactly at the limit is permitted; a larger independent file is rejected. Small resolver stubs and other independent executable fixtures stay within this bound. Fixture-size validation checks existing independent fixtures against it; large real executables use registered links instead of raising the limit.

The checker inspects `.local/bin` entries and their resolved release targets, plus every retained or unpublished release `bin` directory. Inspecting only the published symlink would miss a large copy inside a release.

| Staged entry | Result |
| --- | --- |
| Independent executable at or below 1 MiB | Accepted. |
| Independent executable above 1 MiB | Rejected. |
| Oversized hardlink matching a registered artifact identity | Accepted. |
| Oversized symlink resolving to a registered canonical artifact or verified identity | Accepted. |
| Oversized file with multiple links but no matching registration | Rejected. |
| `.local/bin` symlink concealing an oversized independent release copy | Rejected. |
| Dangling or cyclic link | Rejected. |

A link count greater than one does not establish provenance. Registered artifacts must remain outside release trees, so registering a copied release target cannot legitimize it. Verification reports inspected paths, oversized independent bytes, and verified linked logical bytes separately. Linked logical bytes describe apparent staged sizes, not newly written payload bytes.

## Proposed regression coverage

Required binary-isolation regressions must cover independent copies, valid hardlinks and symlinks, unrelated hardlinks, threshold boundaries, concealed release copies, unpublished releases, dangling links, and cycles. Required linker regressions must cover forced symlink fallback, total failure without copying, malicious destinations, and source-permission preservation.

Required caller coverage must include direct installation and indirect self-heal through profile, recipe, and launch fixtures, plus direct library calls, subprocesses, nested builds, executable overrides, and executable relocation. Resolver-name searches alone do not establish caller coverage.

The implementation pull request must link a caller inventory mapping each direct and indirect installation path to its fixture and regression test. Track that inventory through [issue #2182](https://github.com/rysweet/amplihack-rs/issues/2182); replace this tracking link with the concrete inventory link when available. No completed inventory or passing issue #2182 regression suite is claimed here.

## Proposed write measurement contract

The measurement workload is exactly `cargo test -p amplihack-cli` on both baseline and fixed revisions, with isolated test roots and equivalent cache preparation. The accounting includes descendants and nested Cargo builds, not just the top-level Cargo process.

Dedicated cgroup accounting is preferred. Process-tree accounting records its coverage and limitations, including whether short-lived descendants can escape observation. Both runs retain complete logs, exit status, nested-build activity, revision, Rust toolchain, cache state, filesystem behavior, accounting scope, and material limitations.

Operating-system write counters and logical staging volume are separate measurements. Hardlinks, filesystem caching, reflinks, and accounting gaps can change physical writes without proving that independent copies disappeared. Structural footprint verification supplies that evidence. A failed or incomplete suite remains reported as such and does not establish successful full-suite validation.

Measurement outputs and before/after numbers belong in CI logs and the implementation pull request, not this reference. Link those retained logs and the comparison from [issue #2182](https://github.com/rysweet/amplihack-rs/issues/2182) and add their concrete evidence links here when available. Measurements are required before claiming verified write reduction; none are supplied by this proposed contract.

## Proposed production scope

Production installation optimization remains outside this proposal and is deferred to [issue #2147](https://github.com/rysweet/amplihack-rs/issues/2147). This test policy does not change production copying, reduce resolver size (#2025), optimize profile `/proc` scans (#2042), add timers, or introduce host-specific behavior. The `test-support` executable stays in dedicated test output and is excluded from release artifacts.
