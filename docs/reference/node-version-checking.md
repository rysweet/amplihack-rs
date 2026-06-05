# Node.js Version Checking

**Type**: Reference (Information-Oriented)
**Last Updated**: 2026-06-05
**Since**: Issue #679; launch-time remediation refined for the post-v0.9.77 finalization work

## Overview

The Node.js version checking system ensures that the installed Node.js
runtime meets the minimum version required by downstream tools — specifically
GitHub Copilot CLI, which requires Node.js v24+.

This check is separate from the existing prerequisite *presence* detection
(`check_prerequisites`), which verifies that `node` is installed at all.
Version checking adds a second layer: confirming the installed version is
sufficient for the tool that will use it.

## API Reference

All functions are in `amplihack_utils::prerequisites`.

### `parse_node_major_version(version_output: &str) -> Option<u32>`

Parses the major version number from a `node --version` output string.

**Parameters:**

| Name | Type | Description |
|------|------|-------------|
| `version_output` | `&str` | Raw output from `node --version` (e.g., `"v24.1.0\n"`) |

**Returns:** `Option<u32>` — the major version number, or `None` if the
string cannot be parsed.

**Parsing rules:**

- Trims whitespace and trailing newlines
- Strips optional leading `v` or `V` prefix
- Extracts the first integer before the first `.`
- Returns `None` for empty strings, non-numeric content, or unrecognized formats

**Examples:**

```rust
use amplihack_utils::prerequisites::parse_node_major_version;

assert_eq!(parse_node_major_version("v24.1.0\n"), Some(24));
assert_eq!(parse_node_major_version("v20.19.4"), Some(20));
assert_eq!(parse_node_major_version("24.0.0"), Some(24));
assert_eq!(parse_node_major_version(""), None);
assert_eq!(parse_node_major_version("not-a-version"), None);
```

### `check_node_minimum_version(min_major: u32) -> Result<(), NodeVersionError>`

Runs `node --version`, parses the output, and returns an error if the
installed version is below `min_major`.

**Parameters:**

| Name | Type | Description |
|------|------|-------------|
| `min_major` | `u32` | Minimum required major version (e.g., `24`) |

**Returns:** `Ok(())` when the detected Node.js major version is at least
`min_major`.

Returns `Err(NodeVersionError)` when:

- The `node` binary is not found on `PATH`
- Node.js exists but its version cannot be detected by the prerequisite checker
- The detected major version is below `min_major`

If a version string reaches the parser but has an unrecognized format, the
function logs a warning and returns `Ok(())` to avoid blocking unusual Node.js
builds. Most missing or undetectable installations are still surfaced as
`NodeVersionError`.

### `NodeVersionError`

Error type returned by `check_node_minimum_version`.

```rust
#[derive(Debug, Error)]
pub enum NodeVersionError {
    InsufficientVersion {
        found: u32,
        minimum: u32,
        install_hint: String,
    },
    VersionUndetectable {
        install_hint: String,
    },
    NotFound {
        install_hint: String,
    },
}
```

**Variants:**

| Variant | Description |
|-------|-------------|
| `InsufficientVersion` | Node.js is installed, but the major version is below the required minimum. |
| `VersionUndetectable` | The `node` binary was found, but the prerequisite checker could not extract a version. |
| `NotFound` | The `node` binary was not found on `PATH`. |

## Integration Points

### `amplihack copilot` (bootstrap.rs)

**Behavior:** launch-time remediation. `amplihack copilot` requires Node.js
v24+. If the system Node.js is sufficient, launch continues without changes. If
Node.js is missing or too old, `amplihack` attempts to install a managed Node.js
runtime under `~/.amplihack/runtimes/` and prepends its `bin` directory to
`PATH` for the current launch.

**Location:** Called in the `"copilot"` arm of `ensure_tool_ready()`, before
`ensure_copilot_home_staged()`.

```
ensure_tool_ready("copilot")
├── non-interactive guard (CI skip)
├── check_required_tools()
├── ensure_framework_installed()
├── ensure_recipe_runner_up_to_date()
├── ensure_node_for_copilot()  ← auto-install managed Node.js when needed
└── ensure_copilot_home_staged()
```

Auto-install is supported for Linux x86_64, macOS x86_64, and macOS aarch64
using official Node.js `.tar.xz` distributions. Unsupported platforms and
non-interactive environments return a hard error with manual installation
guidance instead of attempting a download.

### `amplihack install` (install/mod.rs)

**Behavior:** warning only. `amplihack install` prints a prominent message but
does not fail the install, since it configures many tools beyond just Copilot.

**Location:** Called at the Copilot plugin registration step, before
`register_copilot_plugin()`.

```
local_install(repo_root)
├── ...phases 1-5...
├── 🐙 Configuring GitHub Copilot CLI plugin:
│   ├── check_node_minimum_version(24)  ← warning on missing or insufficient Node.js
│   └── register_copilot_plugin()
├── ...phases 6-8...
```

If the version check fails, the warning is printed with the `⚠️` prefix and
includes upgrade instructions. The install continues and the Copilot plugin is
still registered. Launch-time `amplihack copilot` may later remediate Node.js by
installing a managed runtime on supported interactive hosts.

## Configuration

The minimum version (`24`) is passed as a parameter at each check call site. The
Copilot launch path also exposes the same requirement as
`NODE_MINIMUM_MAJOR = 24` and downloads `NODE_AUTO_INSTALL_VERSION` when managed
Node.js is required.

No environment variables, config files, or feature flags bypass the version
requirement. Use a compatible system Node.js or let `amplihack copilot` install
the managed runtime on a supported interactive host.

## Security

- **No new subprocess spawning.** `check_node_minimum_version` reuses the
  existing `check_tool("node")` infrastructure in `prerequisites.rs`, which
  runs `node --version` via `ProcessManager` with the standard 5-second timeout.
- **`parse_node_major_version` is a pure function** — `fn(&str) -> Option<u32>`.
  No shell expansion, no `eval`, no environment interaction.
- **Explicit missing-tool errors** — missing Node.js and undetectable versions
  produce typed errors so launch-time remediation can make a clear decision.
- **Conservative parser fallback** — if an unusual version string reaches the
  parser directly, the check logs a warning and avoids blocking that launch.
- **Error messages are safe** — the `install_hint` is sourced from the
  sanitized `install_hint()` function (hardcoded per-platform strings). No
  user input is interpolated into error messages.

## Testing

### Unit tests (`prerequisites_tests.rs`)

| Test | Input | Expected |
|------|-------|----------|
| Standard v-prefix | `"v24.1.0\n"` | `Some(24)` |
| No v-prefix | `"20.19.4"` | `Some(20)` |
| Major only | `"v24"` | `Some(24)` |
| Two-part version | `"v24.1"` | `Some(24)` |
| Trailing whitespace | `"v22.5.1  \n"` | `Some(22)` |
| Empty string | `""` | `None` |
| Whitespace only | `"  \n"` | `None` |
| Non-numeric | `"not-a-version"` | `None` |
| Leading text | `"node v24.1.0"` | `None` (conservative) |

### Integration behavior

The version check is tested indirectly through bootstrap and install
integration tests. The empty-config recovery path is tested via unit tests
on `register_in_config` in `copilot_plugin.rs`:

| Config state | Recovery behavior |
|-------------|-------------------|
| File missing | Created as `{}` |
| Empty file (0 bytes) | Treated as `{}` |
| Whitespace-only (`"  \n"`) | Treated as `{}` |
| Valid JSON | Parsed normally |
| Invalid JSON (non-empty) | Parse error returned |

## Related

- [Prerequisites](../PREREQUISITES.md) — tool presence detection
- [Prerequisite Checking System](prerequisite-checking.md) — full detection API
- [Install Command Reference](install-command.md) — install phases
- [Troubleshooting: Node Version](../troubleshooting/node-version-copilot-cli.md) — user-facing fix guide
