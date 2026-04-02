# Contributing to amplihack-rs

## Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| Rust | 1.85+ | Core development (edition 2024) |
| pre-commit | latest | Git hooks for formatting, linting, testing |

## Setup

```bash
git clone https://github.com/rysweet/amplihack-rs.git
cd amplihack-rs
cargo build
cargo test --workspace --skip fleet_probe --skip kuzu --skip fleet::fleet_local --skip memory::kuzu
pip install pre-commit
pre-commit install
```

## Pre-commit hooks

Each commit automatically runs:

- **`cargo fmt --all`** — formatting check
- **`cargo clippy -- -D warnings`** — lint with zero warnings
- Standard hooks: trailing-whitespace, end-of-file-fixer, check-yaml, check-toml, check-merge-conflict

On `pre-push`, the full workspace test suite runs (skipping heavy integration tests).

## Testing

```bash
# Fast: skip kuzu C++ build and fleet probes
cargo test --workspace --skip fleet_probe --skip kuzu --skip fleet::fleet_local --skip memory::kuzu

# Single crate
cargo test -p amplihack-cli
cargo test -p amplihack-hooks

# Coverage
cargo llvm-cov --lib --workspace --skip fleet_probe --skip kuzu
```

## Pull request process

1. Create a feature branch from `main`
2. Ensure `cargo fmt`, `cargo clippy -- -D warnings`, and tests pass
3. All PRs must pass the **merge-criteria** skill before merge
4. All PRs must have a **quality-audit** cycle (3+ rounds)
5. Squash merge preferred

## Code standards

- All modules < 400 lines
- Test coverage ≥ 70%
- All public items documented with `///` doc comments
- No `unwrap()` in library code — use `?` or explicit error handling
- No deferred technical debt
