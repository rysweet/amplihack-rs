# Rust Code Quality Standards

**Type**: Reference (Information-Oriented)

Code quality standards, lint rules, and idioms enforced across all `amplihack-rs`
Rust crates. These standards are checked automatically in CI and must pass before
any PR is merged.

---

## Quality Gates

Every PR must satisfy all four gates before merging:

| Gate | Command | Expectation |
|------|---------|-------------|
| Lint | `cargo clippy -- -D warnings` | Zero warnings, zero errors |
| Format | `cargo fmt --all --check` | No format drift |
| Tests | `cargo test --workspace` | All tests pass, no regressions |
| Dead code | Enforced via clippy | No unused items without annotation |

Run locally in sequence:

```bash
cargo fmt --all
cargo clippy -- -D warnings
cargo test --workspace
```

---

## Clippy Lint Rules

### Zero-warnings policy

The entire workspace compiles with `-D warnings`, which promotes every clippy
warning to a hard error. No warnings are permitted without an explicit,
documented `#[allow(...)]` attribute.

### Inlined format arguments (`uninlined_format_args`)

Use inlined variable syntax in all format strings:

```rust
// ✅ correct
let msg = format!("{name} failed with {code}");
tracing::warn!("{path} not found");

// ❌ wrong — triggers uninlined_format_args
let msg = format!("{} failed with {}", name, code);
tracing::warn!("{} not found", path);
```

This applies to `format!`, `println!`, `eprintln!`, `write!`, `writeln!`,
`tracing::info!`, `tracing::warn!`, `tracing::error!`, and all similar macros.

To batch-fix the entire workspace at once:

```bash
cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
cargo fmt --all
```

---

## Design Patterns

### Associated functions vs methods

Prefer associated functions (no `self`) for pure transformations that do not
need instance state. Use `&self` only when the function actually reads from
`self`.

```rust
// ✅ correct — pure transformation, no instance state needed
impl SettingsGenerator {
    pub fn merge_settings(base: &Value, overrides: &Value) -> Value {
        // ...
    }
}

// Call site
let merged = SettingsGenerator::merge_settings(&base, &overrides);

// ❌ wrong — unnecessarily borrows self
impl SettingsGenerator {
    pub fn merge_settings(&self, base: &Value, overrides: &Value) -> Value {
        // never reads self fields
    }
}
```

The `only_used_in_recursion` clippy lint will flag methods where `self` is
passed through recursion but never actually read. Convert these to associated
functions.

### Architectural constants

Internal constants that exist to document design intent may not be used
directly in code but still belong in the source as authoritative documentation
of system behavior. Annotate them with `#[allow(dead_code)]` and a comment
explaining their purpose:

```rust
/// Timeout for Docker CLI commands.
///
/// Not referenced in call sites because `std::process::Command` does not
/// support native timeouts; enforced at a higher layer. Kept here as the
/// authoritative record of the intended limit.
#[allow(dead_code)]
const DOCKER_TIMEOUT: Duration = Duration::from_secs(5);
```

Do **not** remove such constants to silence the warning. The constant documents
intent; the annotation acknowledges that it is intentionally unused in code.

Do **not** use workarounds like `const _: Type = CONSTANT;` to trick the
compiler into accepting the constant — this is fragile and confusing.

---

## Error Handling

See [`CONTRIBUTING_RUST.md`](../../CONTRIBUTING_RUST.md) for the error strategy
table. In summary:

| Crate type | Error crate | Pattern |
|------------|-------------|---------|
| Library (`amplihack-*` without binary) | `thiserror` | Typed `enum Error` |
| Binary / CLI | `anyhow` | `.context("what we were doing")` |

Never use `Box<dyn Error>` or `unwrap()` in library crates.

---

## Format Style

`rustfmt.toml` sets the workspace format configuration. Key settings:

| Option | Value | Effect |
|--------|-------|--------|
| `edition` | `2024` | Rust 2024 edition formatting |
| `max_width` | `100` | Line length cap |

Run `cargo fmt --all` to apply; `cargo fmt --all --check` to verify.

---

## CI Integration

The quality gates run in `.github/workflows/` on every push and PR:

```
lint job      → cargo clippy -- -D warnings
fmt job       → cargo fmt --all --check
test job      → cargo test --workspace
```

All three must pass for the merge button to be enabled. There are no optional
quality gates — all are required.

---

## Common Fixes Reference

| Warning | Root cause | Fix |
|---------|-----------|-----|
| `uninlined_format_args` | `format!("{}", x)` | Change to `format!("{x}")` |
| `only_used_in_recursion` | `&self` method that never reads self | Convert to associated function |
| `dead_code` on constant | Architectural constant with no call site | Add `#[allow(dead_code)]` with comment |
| `needless_pass_by_ref_mut` | `&mut T` param that isn't mutated | Change to `&T` |
| `clippy::redundant_closure` | `\|x\| f(x)` | Replace with `f` |
| `clippy::match_wildcard_for_single_variants` | `_ =>` in exhaustive enum match | Add explicit arms |
