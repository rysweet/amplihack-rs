# amplihack-rs Documentation

amplihack-rs is the Rust implementation of the amplihack CLI. It replaces the Python-based installer with a native binary that bootstraps the complete amplihack environment in a single command.

## Contents

### How-To Guides

- [Install amplihack for the First Time](./howto/first-install.md) — Bootstrap from scratch, including Python validation, binary deployment, and hook registration
- [Install from a Local Repository](./howto/local-install.md) — Install without network access using a local checkout
- [Uninstall amplihack](./howto/uninstall.md) — Cleanly remove all installed files, binaries, and hook registrations
- [Enable Shell Completions](./howto/enable-shell-completions.md) — Activate tab-completion for amplihack in bash, zsh, fish, or PowerShell
- [Diagnose Problems with amplihack doctor](./howto/diagnose-with-doctor.md) — Fix each of the 7 health checks that `amplihack doctor` can report as failed
- [Resolve kuzu Linker Errors](./howto/resolve-kuzu-linker-errors.md) — Diagnose and fix `undefined reference` errors caused by `cxx`/`cxx-build` version mismatch

### Reference

- [amplihack install](./reference/install-command.md) — Full CLI reference for the `install` and `uninstall` commands
- [amplihack completions](./reference/completions-command.md) — Generate shell completion scripts for bash, zsh, fish, and PowerShell
- [amplihack doctor](./reference/doctor-command.md) — All 7 system health checks, exit codes, output format, and security properties
- [Windows Build Target](./reference/windows-build-target.md) — x86_64-pc-windows-msvc support: artifacts, CI pipeline, platform behaviour, and known limitations
- [Install Manifest](./reference/install-manifest.md) — Schema and semantics of the uninstall manifest written at install time
- [Hook Specifications](./reference/hook-specifications.md) — Canonical table of all 7 Claude Code hooks registered by amplihack
- [Binary Resolution](./reference/binary-resolution.md) — How `amplihack` locates the `amplihack-hooks` binary at install time

### Concepts

- [Bootstrap Parity](./concepts/bootstrap-parity.md) — Why the Rust CLI replicates the Python installer's first-install flow and what that means for users
- [Idempotent Installation](./concepts/idempotent-installation.md) — How repeated installs are safe and how existing hook registrations are updated in place
- [The cxx/cxx-build Version Contract](./concepts/cxx-version-contract.md) — Why `cxx` and `cxx-build` must share the same minor version and how a mismatch produces linker errors

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

# Check system prerequisites
amplihack doctor

# Enable tab-completion (bash example)
amplihack completions bash > ~/.local/share/bash-completion/completions/amplihack
```

## Related

- [README](../README.md) — Architecture overview and design principles
- [CONTRIBUTING_RUST.md](../CONTRIBUTING_RUST.md) — Developer setup, build targets, test harness
