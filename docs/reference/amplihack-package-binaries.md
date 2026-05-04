# amplihack Package — Multiple Binaries and `default-run`

The `amplihack` Cargo package (`bins/amplihack/`) contains two `[[bin]]`
targets:

| Binary | Entry point | Purpose |
| --- | --- | --- |
| `amplihack` | `src/main.rs` | Main CLI — the primary user-facing binary |
| `scan-invisible-chars` | `src/bin/scan_invisible_chars.rs` | Development utility for detecting invisible Unicode characters |

## `default-run`

```toml
# bins/amplihack/Cargo.toml
[package]
name = "amplihack"
default-run = "amplihack"
```

`default-run = "amplihack"` is set in `bins/amplihack/Cargo.toml` so that
`cargo run --package amplihack` unambiguously runs the `amplihack` binary.
Without this directive, Cargo requires `--bin amplihack` whenever the package
contains more than one `[[bin]]` target.

## When this matters

### Integration tests

The `issue_538_install_completeness` integration test drives `amplihack
install` via `cargo run --package amplihack`:

```rust
Command::new(env!("CARGO"))
    .arg("run")
    .arg("--quiet")
    .arg("--package")
    .arg("amplihack")
    .arg("--")
    .arg("install")
    // ...
```

This pattern — `cargo run --package amplihack -- <subcommand>` — requires
`default-run` to be set. If `default-run` is absent and a second `[[bin]]`
target exists, Cargo exits with:

```
error: `cargo run` could not determine which binary to run. Use the `--bin` option to specify a binary, or the `default-run` manifest key.
```

### Developer workflow

```sh
# Run the amplihack CLI from the workspace root:
cargo run --package amplihack -- --help

# Run the scan-invisible-chars utility explicitly:
cargo run --package amplihack --bin scan-invisible-chars -- path/to/file.rs
```

## Adding a new `[[bin]]` target

If a new binary target is added to `bins/amplihack/Cargo.toml`, the
`default-run = "amplihack"` entry ensures existing tooling and integration
tests continue to work without changes.

New binaries that are not the primary CLI **must not** replace or remove the
`default-run` directive.

## Related

- [`docs/reference/install-completeness.md`](install-completeness.md) —
  install verification contract and integration test behaviour
- [`docs/reference/binary-resolution.md`](binary-resolution.md) — runtime
  hook-binary resolution sequence used during `amplihack install`
