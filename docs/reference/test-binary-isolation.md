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
- [Status of the issue #2182 wanted items](#status-of-the-issue-2182-wanted-items)
- [Known limits](#known-limits)

## Staging

`stage_binary()` validates every source as an executable regular file before it writes anything. A source larger than `LINK_THRESHOLD` (1 MiB) is registered and linked into the release `bin` directory instead of being copied:

1. A hardlink is tried first.
2. If the hardlink fails, a symlink is tried.
3. If both fail, staging returns an error. It never falls back to a copy.

The link destination must lie inside the release `bin` directory, must not already exist and must not contain parent traversal. After linking, the destination must match the registered source's filesystem identity (device and inode on Unix).

Release commit normally sets file modes in a release. Linked test binaries share their inode with the build artifact, so the commit skips them and the source's permissions are left unchanged.

Sources at or below 1 MiB, such as shell stubs, are still copied and made executable.

## Resolver discovery

Test builds use a private inert resolver stub, created once per process in a temporary directory, in place of sibling or PATH discovery. The stub exits with an error if it is ever run, so a test that needs real asset resolution fails instead of silently using a host resolver.

The production discovery is `binary::discover_resolver()`, compiled in every build. `deploy_binaries_into()` calls it only in a non-test build, so the unit tests in `issue_2182_binary_isolation::production_resolver_discovery` call it directly: an executable sibling wins, a non-executable sibling falls back to `PATH`, and `PATH` is searched only when the install mode allows it.

## Subprocess fixtures

`tests/support/amplihack_bin.rs` builds `amplihack` and `amplihack-hooks` with the `amplihack-cli/test-support` feature into `<target>/amplihack-test-support`, so the feature never reaches the developer's own `target/debug`. The folder is fixed and kept until `cargo clean`. Every test binary and every later run builds into it, so dependencies such as `lbug` are compiled once and reused, and cargo's lock serialises builds that start together. The binaries are copied out of it before any test runs them.

`AMPLIHACK_TEST_BINARY` still overrides the binary and is used as given. It is not the test-support build, so an install test run with it uses production discovery and copies a resolver found beside it into each fake home. A note on stderr says so. Leave it unset to use the protected build.

`tests/support/isolated_amplihack.rs` gives each child a private `HOME` and clears the application and profile root overrides so they derive from that home. It replaces each `PATH` directory that holds an `amplihack-asset-resolver` with a private directory of symlinks to that directory's other entries, so `cargo`, `sccache` or `recipe-runner-rs` installed beside a resolver stay on the child's `PATH` in the same order.

## Internal support modules

| Module | Responsibility |
| --- | --- |
| `commands::install::test_support` | Provides the private resolver stub used by test builds. |
| `test_support::artifacts` | Validates executable sources, records their identities, and creates confined links with hardlink then symlink fallback. |

These are test-support modules, not public application APIs.

## Tests

- `commands::install::tests::issue_2182_binary_isolation` covers symlink fallback after a failed hardlink, total link failure without a copy, rejected destinations outside the staging root, rejected sources, a replaced registered source, and final release staging of large `amplihack`, `amplihack-asset-resolver` and `amplihack-hooks` sources as hardlinks with unchanged permissions.
- `commands::install::tests::issue_2182_binary_isolation::production_resolver_discovery` covers the production sibling and `PATH` discovery.
- `tests/issue_2182_binary_isolation.rs` checks that the subprocess fixture keeps application roots inside the private home, that its `PATH` offers no `amplihack-asset-resolver`, and that tools sharing a directory with a resolver, including through a symlink, stay reachable.
- `tests/issue_1955_install_cli.rs::an_install_leaves_no_large_independent_copy_in_the_fake_home` runs a real install into a fake home and checks that nothing reached from its `.local/bin` is an independent copy over 1 MiB, and that the staged resolver is the stub.

Run them with:

```sh
cargo test -p amplihack-cli --lib issue_2182_binary_isolation
cargo test -p amplihack-cli --test cli_integration issue_2182_binary_isolation
cargo test -p amplihack-cli --test cli_integration an_install_leaves_no_large_independent_copy
```

## Status of the issue #2182 wanted items

| Item | Status |
| --- | --- |
| 1. No real binaries copied into fake homes | Met in test builds by the resolver stub and by linking sources over 1 MiB. |
| 2. Production linking | Not changed here. Left to [issue #2147](https://github.com/rysweet/amplihack-rs/issues/2147). |
| 3. Check that a fake home's `.local/bin` holds no large copy | Partly met. One test, `an_install_leaves_no_large_independent_copy_in_the_fake_home`, checks one real install. Other fixtures that build fake homes are not checked. [Issue #3334](https://github.com/rysweet/amplihack-rs/issues/3334) tracks a check in the shared test support. |
| 3. Before-and-after bytes written by `cargo test -p amplihack-cli` | Not measured. The repository's own tools do not report it. [Issue #3334](https://github.com/rysweet/amplihack-rs/issues/3334) tracks it. |
| 4. No timers; tests never touch the real home | Met. |

## Known limits

- A test build always stages the inert stub. No install test through the library or the `test-support` binary can stage a real resolver, and `AMPLIHACK_TEST_BINARY` reaches production discovery but copies. [Issue #3342](https://github.com/rysweet/amplihack-rs/issues/3342) tracks a test-build way to link a chosen real resolver.
- The `PATH` shadow directory is a snapshot of each shadowed directory's entries at first use. A tool installed into that directory later in the same test process is not visible to children.
