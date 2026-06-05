# Reference: `NodeVersionError` Enum

Technical reference for the `NodeVersionError` enum in
`crates/amplihack-utils/src/prerequisites.rs`.

## Variants

```rust
pub enum NodeVersionError {
    InsufficientVersion { found: u32, minimum: u32, install_hint: String },
    VersionUndetectable { install_hint: String },
    NotFound { install_hint: String },
}
```

### `NotFound`

The `node` binary was not found on `PATH` at all.

**Display:**
```
Node.js is not installed — the `node` binary was not found on PATH.
Install with: <platform-specific hint>
```

**When returned:** `check_node_minimum_version()` runs `node --version`
and the process fails to start (binary not found). The `result.found`
field is `false`.

**Caller behavior:** `ensure_node_for_copilot()` triggers auto-install.
`install/mod.rs` calls treat this as a prerequisite failure and surface
the error message.

### `InsufficientVersion`

The `node` binary exists but its version is below the required minimum.

**Display:**
```
Node.js v16 is installed but v24+ is required.
Upgrade with: <platform-specific hint>
```

**When returned:** `check_node_minimum_version()` successfully runs
`node --version`, parses a valid semver, and the major version is less
than the required minimum.

### `VersionUndetectable`

The `node` binary exists and ran, but its version output could not be
parsed.

**Display:**
```
Could not determine the installed Node.js version.
Install with: <platform-specific hint>
```

**When returned:** `check_node_minimum_version()` runs `node --version`
successfully (binary found, exit code 0 or non-zero with output) but the
stdout does not contain a parseable version string.

## Usage Patterns

### Check and branch

```rust
match check_node_minimum_version(24) {
    Ok(()) => { /* system node is sufficient */ }
    Err(NodeVersionError::NotFound { .. }) => { /* no node at all */ }
    Err(NodeVersionError::InsufficientVersion { found, minimum, .. }) => {
        eprintln!("Node {found} < {minimum}");
    }
    Err(NodeVersionError::VersionUndetectable { .. }) => {
        eprintln!("Cannot determine node version");
    }
}
```

### Check as boolean gate for Copilot launch

```rust
if check_node_minimum_version(24).is_ok() {
    // Node is present and sufficient — skip auto-install
} else {
    // Any error variant — trigger auto-install
    ensure_node_for_copilot()?;
}
```

All three `Err` variants correctly trigger the auto-install path when
used with `.is_ok()` / `.is_err()` in the Copilot launch path. Install-time
callers use the same errors for warnings only.

## `node_platform_triple()`

Returns the Node.js download platform triple for the current OS, or
`None` if auto-install is not supported on this platform.

| Target OS | Return value |
|-----------|-------------|
| Linux x86_64 | `Some(("linux", "x64"))` |
| macOS x86_64 | `Some(("darwin", "x64"))` |
| macOS aarch64 | `Some(("darwin", "arm64"))` |
| Windows | `None` |

Windows returns `None` because Node.js distributes Windows builds as
`.zip` files, and the auto-install extraction code uses `tar -xJf`
which only handles `.tar.xz`. This is a deliberate design choice — the
caller receives `None` and shows a clear "auto-install not supported on
this platform" message.

## Test Coverage

| Test | Verifies |
|------|----------|
| `node_version_error_not_installed_displays_actionable_message` | `NotFound` variant contains "not installed" and the install hint |
| `node_version_error_not_found_is_distinct` | `VersionUndetectable` variant message differs from `NotFound` |
| `node_platform_triple_returns_known_platforms` | Returns values for `linux` and `darwin` only |
| `check_node_minimum_version` tests | End-to-end version checking |

## See Also

- [Node.js Runtime Auto-Install](../concepts/node-runtime-auto-install.md) — how auto-install uses these errors
- [Prerequisites module](../../crates/amplihack-utils/src/prerequisites.rs) — source code
