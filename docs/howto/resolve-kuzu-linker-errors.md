# Resolve LadybugDB Linker Errors

`cargo build` can fail with `undefined reference` linker errors when `cxx` and `cxx-build` resolve to different minor versions. This guide walks through diagnosis and the one-command fix.

## Contents

- [When this guide doesn't apply](#when-this-guide-doesnt-apply)
- [Recognise the error](#recognise-the-error)
- [Diagnose the version mismatch](#diagnose-the-version-mismatch)
- [Fix: pin cxx-build to 1.0.138](#fix-pin-cxx-build-to-10138)
- [Verify the fix](#verify-the-fix)
- [Prevent recurrence](#prevent-recurrence)
- [Related](#related)

## When this guide doesn't apply

This guide is specific to `cxxbridge1$` symbol errors from the LadybugDB (formerly Kuzu) library. If your linker error looks different, this fix will not help:

| Symptom | Look elsewhere |
|---|---|
| Error symbol does **not** contain `cxxbridge1$` | Different FFI or C++ ABI issue |
| Error comes from a crate other than `lbug` | That crate's own FFI version mismatch |
| `undefined reference` to a non-`cxxbridge` symbol | Missing `pkg-config` library or `-sys` crate issue |
| Compilation error (not a linker error) | Unrelated — check the compiler message |

For general Rust linker errors see the [rustc error index](https://doc.rust-lang.org/error_codes/error-index.html).

## Recognise the error

The linker error looks like this:

```
error: linking with `cc` failed: exit status: 1
  = note: /usr/bin/ld: /tmp/.../libkuzu.a(cxxbridge.o): undefined reference
          to `cxxbridge1$box$kuzu$Database$alloc'
          /usr/bin/ld: /tmp/.../libkuzu.a(cxxbridge.o): undefined reference
          to `cxxbridge1$box$kuzu$Connection$alloc'
          collect2: error: ld returned 1 exit status
```

Key indicators:
- Error mentions `cxxbridge` symbols
- Error comes from the LadybugDB library, not from your code
- Build succeeds in CI but fails locally (or vice versa)

## Diagnose the version mismatch

Check whether `cxx` and `cxx-build` are on the same minor version:

```sh
grep -A1 '^name = "cxx"' Cargo.lock
grep -A1 '^name = "cxx-build"' Cargo.lock
```

Expected healthy output — both versions must match:

```
name = "cxx"
version = "1.0.138"
name = "cxx-build"
version = "1.0.138"
```

Unhealthy output — versions differ:

```
name = "cxx"
version = "1.0.138"
name = "cxx-build"
version = "1.0.194"   ← mismatch: symbols embed the minor version token
```

If the versions differ, proceed to the fix below. See [The cxx/cxx-build Version Contract](../concepts/cxx-version-contract.md) for an explanation of why this matters.

## Fix: pin cxx-build to 1.0.138

Run a single targeted `cargo update` that sets `cxx-build` to the same minor version as `cxx`:

```sh
cargo update -p cxx-build --precise 1.0.138
```

This updates only the `cxx-build` entry in `Cargo.lock`. Nothing else changes — no `Cargo.toml` edits, no source changes.

Confirm:

```sh
grep -A1 '^name = "cxx-build"' Cargo.lock
# name = "cxx-build"
# version = "1.0.138"
```

## Verify the fix

```sh
# Build must exit 0 with no linker errors
cargo build

# Full test suite must pass
cargo test --workspace

# Lock file must be self-consistent
cargo build --locked
```

Expected output from `cargo test --workspace` (abbreviated):

```
running 18 tests
test result: ok. 18 passed; 0 failed; 0 ignored
```

## Prevent recurrence

### Don't run `cargo update` without re-pinning

Running `cargo update` regenerates `Cargo.lock` and may re-introduce the mismatch. After any `cargo update`, re-run the pin:

```sh
cargo update
cargo update -p cxx-build --precise 1.0.138
cargo build --locked   # sanity check
```

### Use `--locked` in CI

Adding `--locked` to CI build commands forces Cargo to use the committed `Cargo.lock` exactly:

```yaml
# .github/workflows/ci.yml (recommended addition)
- run: cargo build --locked --release
- run: cargo test --locked --workspace
```

This catches any lock file drift before it reaches developers.

### Run `cargo audit` after pinning

`cxx-build 1.0.138` is a downgrade from the latest release. Check that the pinned version has no known security advisories:

```sh
cargo audit
```

Expected output when no advisories apply:

```
Fetching advisory database from `https://github.com/rustsec/advisory-db.git`
    Loaded 693 security advisories (as of ...)
Scanning Cargo.lock for vulnerabilities (159 crate dependencies)
    No vulnerabilities found
```

> **Security scope note**: `cxx-build` is a **build-time-only** crate — it runs during `cargo build` and generates C++ glue code. It does **not** appear in the compiled binary or any runtime artifact. Any advisory against `cxx-build` affects the build toolchain, not shipped software.

### Track the upstream issue

The root cause is in the lbug crate's `Cargo.toml`, which specifies `cxx-build = "^1.0"` (open range) instead of `cxx-build = "=1.0.138"` (exact pin to match `cxx`). This is an upstream deficiency — amplihack-rs's `Cargo.lock` pin is a workaround, not a design choice. Once the lbug crate ships a corrected release, the local pin can be removed.


## Related

- [The cxx/cxx-build Version Contract](../concepts/cxx-version-contract.md) — Why the versions must match
- [CONTRIBUTING_RUST.md](https://github.com/rysweet/amplihack-rs/blob/main/CONTRIBUTING_RUST.md) — Build and test commands
- [GitHub Issue #35](https://github.com/rysweet/amplihack-rs/issues/35) — Original report and investigation
- [GitHub PR #43](https://github.com/rysweet/amplihack-rs/pull/43) — The fix
