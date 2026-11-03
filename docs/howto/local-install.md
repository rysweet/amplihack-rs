---
title: Install from a Local Repository
description: Build both native binaries with managed lbug reuse and install framework assets from a local checkout.
doc_type: howto
last_updated: 2026-11-03
review_schedule: as-needed
---

# How to Install from a Local Repository

> **[PLANNED - Implementation Pending]** The lbug wrapper commands below
> describe the proposed issue #1917 integration. The wrapper is not implemented;
> deputy specification approval remains outstanding.

Use `amplihack install --local <PATH>` when you want to install from a local checkout instead of cloning from GitHub. This is useful in air-gapped environments, when testing a development branch, or when you have a slow network connection.

## When to Use This

| Scenario | Command |
|----------|---------|
| Air-gapped machine | `amplihack install --local /mnt/usb/amplihack` (pre-install `recipe-runner-rs`, see below) |
| Testing a dev branch | `amplihack install --local ~/src/amplihack-dev` |
| CI pipeline (pre-cloned) | `amplihack install --local $GITHUB_WORKSPACE` |
| Normal first install | `amplihack install` (no flag, uses bundled assets) |

## Steps

### 1. Obtain a local checkout

```sh
git clone https://github.com/rysweet/amplihack-rs.git
cd amplihack-rs
```

### 2. Build the native binaries

From the checkout root, build and install both binaries through the shared
lbug integration:

```bash
scripts/cargo-with-lbug.sh install --locked --path bins/amplihack
scripts/cargo-with-lbug.sh install --locked --path bins/amplihack-hooks
```

Expected result: `amplihack` and `amplihack-hooks` are installed in Cargo's
configured installation root. Matching verified static lbug artifacts skip
bundled CMake and core C++ compilation; the FFI shim still compiles. Missing
artifacts or publication credentials do not prevent a source build. Keep a
C/C++ toolchain and cmake available for fallback.

No manual lbug path exports are needed. Explicit upstream overrides retain
their behavior; `LBUG_SHARED` must be unset for managed static reuse. See the
[cache configuration reference](../reference/lbug-prebuilt-cache.md#configuration).
For offline builds, add `--offline` and pre-provision Cargo dependencies and
the toolchain. The helper makes no network requests; an absent trusted artifact
uses the bundled source instead.

### 3. Run install with --local

`--local` reads the framework bundle from the given checkout and does not
contact GitHub for it, but the recipe-runner phase still runs
`cargo install --git https://github.com/rysweet/amplihack-recipe-runner` when
`recipe-runner-rs` is missing, and that needs network access. On an air-gapped
host, build `recipe-runner-rs` elsewhere and either place it on `PATH`
(`~/.cargo/bin` or `~/.local/bin`) or point `RECIPE_RUNNER_RS_PATH` at it
before installing. If the phase does fail, the staged assets already have an
uninstall manifest: provision the binary and re-run install, or run
`amplihack uninstall`.

```sh
amplihack install --local "$PWD"
```

### Alternative: execute the local Rust checkout through npm

When you want npm/npx to invoke the Rust CLI from a local checkout of
`amplihack-rs`, point `--package` at the repository root:

```sh
npx --yes --package="$PWD" -- amplihack install --local "$PWD"
```

That wrapper path uses the same `amplihack` bin name, provisions
`amplihack` + `amplihack-hooks`, and then delegates to the native Rust CLI.
If a matching application release archive is unavailable, the wrapper builds
from the packaged Cargo workspace through `scripts/cargo-with-lbug.sh`.
The distributed source includes the helper, policies, provenance, lockfiles,
and complete pinned lbug compatibility source. These files are required even
on platforms without a published application binary.

The uvx bootstrap resolves the selected revision once, obtains a complete
owned checkout of that revision, and uses its wrapper to install both binaries.
It does not build the two packages from separately resolved moving revisions.
Both source installers reuse matching lbug artifacts by default and retain
source fallback when acquisition or publication is unavailable. Prebuilt
application archives do not need a lbug runtime cache.

The `--local` flag explicitly specifies the framework source directory. Since issue #254, `amplihack install` (without `--local`) uses bundled framework assets from the amplihack-rs source tree instead of downloading from GitHub. The `--local` flag is still useful for testing a development branch or using a custom framework checkout. All other phases (binary deployment, asset staging, hook wiring) run identically.

### 4. Verify

```sh
amplihack --version
# Expected: amplihack-rs followed by the installed checkout's version.

# Confirm hooks are registered
grep -c "amplihack-hooks" ~/.claude/settings.json
# Should print 5 (one entry per binary-subcommand hook)
```

## Path Requirements

The path passed to `--local` must:

- Exist and be a directory
- Contain a `.claude` directory (either at `<PATH>/.claude` or `<PATH>/../.claude`)

Symlinks within the local repository are skipped with a warning — they are not followed during asset staging. If your checkout uses internal symlinks, the linked targets will not be staged; copy the files directly instead. Device, socket, and FIFO entries are skipped silently.

The installer canonicalizes the root path before staging to prevent traversal outside the given directory.

## Build without deploying binaries to PATH

To test the current checkout before installing its binaries:

```bash
scripts/cargo-with-lbug.sh build --release --locked \
    --bin amplihack --bin amplihack-hooks
```

With Cargo's default target directory, run the resulting CLI directly:

```bash
./target/release/amplihack --version
# Expected: amplihack-rs followed by this checkout's version.
```

## See Also

- [Install amplihack for the First Time](./first-install.md) — standard network install
- [amplihack install reference](../reference/install-command.md) — all flags
- [Prebuilt lbug Cache Reference](../reference/lbug-prebuilt-cache.md) — supported build paths, configuration, and fallback
- [Reuse lbug Across Two Clean Cargo Targets](../tutorials/lbug-cache-reuse.md) — verify reuse independently of a Cargo target
