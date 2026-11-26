---
title: Test binary isolation
description: How test builds stage executables into fake homes without copying large binaries.
type: reference
issue: 2182
---

# Test binary isolation

This reference describes how test builds of `amplihack-cli` keep large executables out of fake homes ([issue #2182](https://github.com/rysweet/amplihack-rs/issues/2182)). Before this change, every fake-home installation copied the real `amplihack-asset-resolver` (about 590 MB) and the other staged binaries into its private home.

All of the behaviour below is compiled only under `cfg(any(test, feature = "test-support"))`. Production installation keeps its existing discovery and copy behaviour; a production change is left to [issue #2147](https://github.com/rysweet/amplihack-rs/issues/2147).

## Contents

- [Staging](#staging)
- [Resolver discovery](#resolver-discovery)
- [Subprocess fixtures](#subprocess-fixtures)
- [Internal support modules](#internal-support-modules)
- [Tests](#tests)

## Staging

`stage_binary()` validates every source as an executable regular file before it writes anything. A source larger than `LINK_THRESHOLD` (1 MiB) is registered and linked into the release `bin` directory instead of being copied:

1. A hardlink is tried first.
2. If the hardlink fails, a symlink is tried.
3. If both fail, staging returns an error. It never falls back to a copy.

The link destination must lie inside the release `bin` directory, must not already exist and must not contain parent traversal. After linking, the destination must match the registered source's filesystem identity (device and inode on Unix).

Release commit normally sets file modes in a release. Linked test binaries share their inode with the build artifact, so the commit skips them and the source's permissions are left unchanged.

Sources at or below 1 MiB, such as shell stubs, are still copied and made executable.

## Resolver discovery

Test builds use a private inert resolver stub, created once per process in a temporary directory, before sibling or PATH discovery. The stub exits with an error if it is ever run, so a test that needs real asset resolution fails instead of silently using a host resolver.

## Subprocess fixtures

`tests/support/amplihack_bin.rs` builds `amplihack` and `amplihack-hooks` with the `amplihack-cli/test-support` feature into a private Cargo output directory, so the feature never reaches the developer's own `target`. A set `AMPLIHACK_TEST_BINARY` stops the test with a message, because that binary is not the test-support build.

`tests/support/isolated_amplihack.rs` gives each child a private `HOME`, clears the application and profile root overrides so they derive from that home, and sets an explicit `PATH`.

## Internal support modules

| Module | Responsibility |
| --- | --- |
| `commands::install::test_support` | Provides the private resolver stub used by test builds. |
| `test_support::artifacts` | Validates executable sources, records their identities, and creates confined links with hardlink then symlink fallback. |

These are test-support modules, not public application APIs.

## Tests

- `commands::install::tests::issue_2182_binary_isolation` covers symlink fallback after a failed hardlink, total link failure without a copy, rejected destinations outside the staging root, rejected sources, a replaced registered source, and final release staging of large `amplihack`, `amplihack-asset-resolver` and `amplihack-hooks` sources as hardlinks with unchanged permissions.
- `tests/issue_2182_binary_isolation.rs` checks that the subprocess fixture keeps application roots inside the private home and does not inherit the runner's `PATH`.

Run them with:

```sh
cargo test -p amplihack-cli --lib issue_2182_binary_isolation
cargo test -p amplihack-cli --test main issue_2182_binary_isolation
```
