# Fix the cxx-build Pin CI Failure

The `Verify cxx-build pin` step in CI fails when `Cargo.lock` contains a
`cxx-build` version other than `1.0.138`. This guide explains why the check
exists and how to restore the correct pin.

## Contents

- [When this guide applies](#when-this-guide-applies)
- [What the check does](#what-the-check-does)
- [Why the pin must hold](#why-the-pin-must-hold)
- [Fix: re-pin cxx-build](#fix-re-pin-cxx-build)
- [Verify locally before pushing](#verify-locally-before-pushing)
- [Related](#related)

---

## When this guide applies

You are looking at a failed `check` job in CI and the step named
**Verify cxx-build pin** shows output similar to:

```
ERROR: cxx-build must be pinned to 1.0.138, found 1.0.194
Run: cargo update -p cxx-build --precise 1.0.138
```

This happens when:
- Someone ran `cargo update` (or `cargo update -p <crate>`) and the resolver
  bumped `cxx-build` past `1.0.138`.
- A dependency's `Cargo.toml` was changed in a way that altered the resolution
  of `cxx-build`.
- `Cargo.lock` was deleted and regenerated from scratch.

---

## What the check does

The CI step runs before any Rust toolchain setup:

```yaml
- name: Verify cxx-build pin
  run: |
    version=$(grep -A1 'name = "cxx-build"' Cargo.lock | grep version | head -1 | sed 's/.*"\(.*\)".*//')
    if [ "$version" != "1.0.138" ]; then
      echo "ERROR: cxx-build must be pinned to 1.0.138, found $version"
      echo "Run: cargo update -p cxx-build --precise 1.0.138"
      exit 1
    fi
```

It reads `Cargo.lock` directly — no Rust toolchain, no network access — so it
fails fast before the expensive toolchain download.

The step is the first to run in the `check` job. Because `test` and
`cross-compile` both `needs: check`, a failed pin check blocks the entire
pipeline.

---

## Why the pin must hold

kuzu 0.11.3 pins its runtime dependency exactly:

```toml
cxx = "=1.0.138"
```

The `cxx-build` code generator emits symbols named with the minor version token.
If `cxx-build` is at `1.0.194` but `cxx` is at `1.0.138`, the generated symbols
don't match and the linker fails with `undefined reference to cxxbridge1$...`
errors.

See [The cxx/cxx-build Version Contract](../concepts/cxx-version-contract.md)
for the full explanation.

---

## Fix: re-pin cxx-build

Run this command in the repository root:

```sh
cargo update -p cxx-build --precise 1.0.138
```

This rewrites the `[[package]]` entry for `cxx-build` in `Cargo.lock` without
touching anything else. Stage and commit the updated lockfile:

```sh
git add Cargo.lock
git commit -m "chore: re-pin cxx-build to 1.0.138"
```

---

## Verify locally before pushing

Confirm the pin is in place before pushing:

```sh
grep -A1 'name = "cxx-build"' Cargo.lock
# name = "cxx-build"
# version = "1.0.138"
```

Both lines must appear with `1.0.138`. If the version line is missing or shows
a different value, re-run the `cargo update` command above.

You can also simulate the exact CI check locally:

```sh
version=$(grep -A1 'name = "cxx-build"' Cargo.lock | grep version | head -1 | sed 's/.*"\(.*\)".*//')
echo "cxx-build version: $version"
# cxx-build version: 1.0.138
```

If the output is `1.0.138` the CI step will pass.

---

## Related

- [The cxx/cxx-build Version Contract](../concepts/cxx-version-contract.md) — Why the two crates must be on the same version
- [Resolve kuzu Linker Errors](./resolve-kuzu-linker-errors.md) — Fix `undefined reference` errors from a version mismatch
