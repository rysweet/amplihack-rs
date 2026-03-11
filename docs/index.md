# amplihack-rs Documentation

amplihack-rs is the Rust implementation of the amplihack CLI. It replaces the Python-based installer with a native binary that bootstraps the complete amplihack environment in a single command.

## Contents

### How-To Guides

- [Install amplihack for the First Time](./howto/first-install.md) — Bootstrap from scratch, including Python validation, binary deployment, and hook registration
- [Install from a Local Repository](./howto/local-install.md) — Install without network access using a local checkout
- [Uninstall amplihack](./howto/uninstall.md) — Cleanly remove all installed files, binaries, and hook registrations

### Reference

- [amplihack install](./reference/install-command.md) — Full CLI reference for the `install` and `uninstall` commands
- [Install Manifest](./reference/install-manifest.md) — Schema and semantics of the uninstall manifest written at install time
- [Hook Specifications](./reference/hook-specifications.md) — Canonical table of all 7 Claude Code hooks registered by amplihack
- [Binary Resolution](./reference/binary-resolution.md) — How `amplihack` locates the `amplihack-hooks` binary at install time

### Concepts

- [Bootstrap Parity](./concepts/bootstrap-parity.md) — Why the Rust CLI replicates the Python installer's first-install flow and what that means for users
- [Idempotent Installation](./concepts/idempotent-installation.md) — How repeated installs are safe and how existing hook registrations are updated in place

## Quick Start

```sh
# Build from source
cargo build --release

# Install amplihack (first time)
~/.cargo/bin/amplihack install

# Install from a local clone (no network)
amplihack install --local /path/to/amplihack-clone

# Remove everything amplihack installed
amplihack uninstall
```

## Related

- [README](../README.md) — Architecture overview and design principles
- [CONTRIBUTING_RUST.md](../CONTRIBUTING_RUST.md) — Developer setup, build targets, test harness
