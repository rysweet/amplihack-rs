# How to Install from a Local Repository

Use `amplihack install --local <PATH>` when you want to install from a local checkout instead of cloning from GitHub. This is useful in air-gapped environments, when testing a development branch, or when you have a slow network connection.

## When to Use This

| Scenario | Command |
|----------|---------|
| Air-gapped machine | `amplihack install --local /mnt/usb/amplihack` (pre-install `recipe-runner-rs`, see below) |
| Testing a dev branch | `amplihack install --local ~/src/amplihack-dev` |
| CI pipeline (pre-cloned) | `amplihack install --local $GITHUB_WORKSPACE` |
| Normal first install | `amplihack install` (no flag; uses a same-version local source or fetches the release tag) |
| Staging a tree whose version differs from the binary | `amplihack install --local ~/src/amplihack-rs` |

## Steps

### 1. Obtain a local checkout

```sh
# Clone the repository (amplihack-rs bundles framework assets since #254)
git clone https://github.com/rysweet/amplihack-rs ~/src/amplihack-rs

# Or copy from another machine
rsync -a user@remote:~/src/amplihack ~/src/amplihack
```

### 2. Run install with --local

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
amplihack install --local ~/src/amplihack
```

### Alternative: execute the local Rust checkout through npm

When you want npm/npx to invoke the Rust CLI from a local checkout of
`amplihack-rs`, point `--package` at the repository root:

```sh
npx --yes --package=/path/to/amplihack-rs -- amplihack install
```

That wrapper path uses the same `amplihack` bin name, provisions
`amplihack` + `amplihack-hooks`, and then delegates to the native Rust CLI.
If a matching GitHub release archive is unavailable, the wrapper falls back to
building from the packaged Cargo workspace.

That local-checkout path is also the way to use the wrapper on platforms without
published release archives yet, because the fallback build needs the Rust
workspace and a local Rust toolchain.

The `--local` flag explicitly specifies the framework source directory. Without `--local`, `amplihack install` uses `AMPLIHACK_HOME` or the current checkout only when its version equals the binary's, refuses when it differs, and otherwise fetches the tag of the binary's release. See [Framework source selection](../reference/framework-source-selection.md). All other phases (binary deployment, asset staging, hook wiring) run identically.

### Staging a tree with a different version

`--local` is the only way to stage a tree whose version differs from the binary's, older or newer. Install prints both versions and a warning, then stages the tree:

```text
📦 Using framework source /home/alice/src/amplihack-old (v1.2.90, from --local)
⚠️  This source is v1.2.90 but this binary is v1.2.99. Staging it because --local was given.
```

The stamp then records the binary's version, so self-heal leaves your choice in place. `--local` doesn't skip the [compatibility check](../reference/framework-bundle-compatibility.md).

### 3. Verify

```sh
amplihack --version
# amplihack-rs 0.1.0

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

## Example: Installing a Feature Branch

```sh
# Check out the feature branch
git clone --branch feat/bootstrap-parity https://github.com/rysweet/amplihack-rs \
    ~/src/amplihack-bootstrap-test

# Build the CLI from the same branch (if testing CLI changes)
cd ~/src/amplihack-rs-update
git checkout feat/bootstrap-parity
cargo build --release

# Install from the local checkout
./target/release/amplihack install --local ~/src/amplihack-bootstrap-test
```

## See Also

- [Install amplihack for the First Time](./first-install.md) — standard network install
- [amplihack install reference](../reference/install-command.md) — all flags
- [Framework source selection](../reference/framework-source-selection.md) — how install chooses a source without `--local`
