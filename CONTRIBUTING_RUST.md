# Contributing to amplihack-rs

Rust core runtime for amplihack deterministic infrastructure.

## Prerequisites

- **Rust** 2024 edition (1.85+): `rustup update stable`
- **GNU cross toolchain** (for ARM64 Linux): `sudo apt-get install gcc-aarch64-linux-gnu g++-aarch64-linux-gnu`
- **Python 3.11+** with amplihack installed (for SDK bridge tests)

## Build & Test

```bash
cargo build                        # debug build
cargo build --release              # release build (LTO, stripped)
cargo test --workspace             # all tests
cargo clippy -- -D warnings        # lint (zero warnings policy)
cargo fmt --check                  # format check
```

For local development on disk-constrained machines, prefer the wrapper below.
It keeps build artifacts in `/tmp/amplihack-rs-target-$USER`, prunes stale temp
targets older than 3 days by default, and exposes repo/worktree disk usage:

```bash
scripts/dev-space.sh cargo test --workspace
scripts/dev-space.sh status
scripts/dev-space.sh prune-targets --repo-target
```

### Golden file tests

610 golden test cases validate hook parity with Python:

```bash
cargo test --test golden           # run golden file suite
```

Golden files live in `tests/golden/hooks/{hook_type}/{name}.input.json` + `.expected.json`.
The harness uses semantic JSON comparison with wildcards (`__ANY__`, `__CONTAINS__:substring`).

## Code Patterns

### Error strategy

| Context | Crate | Pattern |
|---------|-------|---------|
| Library crates | `amplihack-types`, `amplihack-state` | `thiserror` custom errors |
| Binary crates | `amplihack`, `amplihack-hooks` | `anyhow` with `.context()` |

Never use `Box<dyn Error>`.

### Panic handler (hooks)

```rust
pub fn run_hook<F>(process: F, policy: FailurePolicy)
where F: FnOnce(HookInput) -> anyhow::Result<HookOutput>
{
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| { ... }));
    match (result, policy) {
        (Err(_), FailurePolicy::Open) => { stdout.write_all(b"{}"); }
        (Err(e), FailurePolicy::Closed) => { /* deny */ }
        ...
    }
}
```

### Atomic file operations

Always use temp file + rename for writes:

```rust
let temp = NamedTempFile::new_in(path.parent())?;
serde_json::to_writer_pretty(temp.as_file(), &data)?;
temp.persist(&path)?;  // atomic rename
```

### File locks

Use `F_SETLK` (non-blocking) + retry with timeout. Never `F_SETLKW` (blocks indefinitely).

### Shell command parsing

```rust
let tokens = shell_words::split(input)?;
let commands = tokens.split(|t| ["&&", "||", ";", "|"].contains(&t.as_str()));
```

### IPC versioning

All `HookOutput` includes `version: 1`. Future protocol changes increment this.

## Workspace Layout

```
crates/
├── amplihack-types/    # IPC boundary types (HookInput, HookOutput, ToolDecision)
├── amplihack-state/    # File ops, locking, env config
├── amplihack-hooks/    # Hook implementations + protocol
└── amplihack-cli/      # CLI commands + launcher
bins/
├── amplihack/          # CLI binary
└── amplihack-hooks/    # Multicall hook binary
tests/
└── golden/             # 610 golden test cases
```

## Cross-Compilation

```bash
# Native (current platform)
cargo build --release

# Linux ARM64
rustup target add aarch64-unknown-linux-gnu
CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
cargo build --release --target aarch64-unknown-linux-gnu

# macOS (from macOS host)
rustup target add aarch64-apple-darwin
cargo build --release --target aarch64-apple-darwin
```

CI builds all 4 targets: `x86_64-linux`, `aarch64-linux`, `x86_64-darwin`, `aarch64-darwin`.

## PR Process

1. Create feature branch from `main`
2. Commit after each completed function
3. Run `cargo clippy -- -D warnings && cargo fmt --check && cargo test`
4. Push and create PR — **never push directly to main**
5. PR after each logical unit (one hook, one crate)
