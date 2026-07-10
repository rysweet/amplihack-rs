---
title: External Dependencies
---
# Key external dependencies (inventory)

Representative crates.io dependencies used across the workspace (see each `Cargo.toml` for authority):

- Async/runtime: `tokio`
- Serialization: `serde`, `serde_json`
- Errors: `anyhow`, `thiserror`
- CLI: `clap`
- Logging: `tracing`
- Regex/text: `regex`

Internal (`amplihack-*`) dependencies are mapped in [compile-deps](../compile-deps/README.md).
