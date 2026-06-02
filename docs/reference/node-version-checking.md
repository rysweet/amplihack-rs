# Node.js Version Checking

**Type**: Reference (Information-Oriented)
**Last Updated**: 2026-06-02
**Since**: Issue #679

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

**Returns:** `Ok(())` if:

- Node.js is not installed (missing-tool is handled separately by `check_prerequisites`)
- Node.js version output cannot be parsed (fail-open to avoid blocking on unusual environments)
- Node.js major version is ≥ `min_major`

Returns `Err(NodeVersionError)` only when the version is *known to be insufficient*.

**Design rationale — fail-open:**

The function intentionally returns `Ok(())` when Node.js is missing or its
version output is unparseable. This avoids duplicating the "is node installed?"
check (already handled by `check_prerequisites`) and prevents false negatives
in unusual environments where `node --version` returns non-standard output.
The only case that produces an error is a *confirmed* insufficient version.

### `NodeVersionError`

Error type returned by `check_node_minimum_version`.

```rust
#[derive(Debug, Error)]
#[error(
    "Node.js v{min_required} or higher is required. \
     Currently installed: v{current_major}.x\n\
     {install_hint}"
)]
pub struct NodeVersionError {
    pub min_required: u32,
    pub current_major: u32,
    pub install_hint: String,
}
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `min_required` | `u32` | The minimum version that was required |
| `current_major` | `u32` | The major version that was detected |
| `install_hint` | `String` | Platform-specific upgrade instructions from `install_hint("node")` |

## Integration Points

### `amplihack copilot` (bootstrap.rs)

**Behavior:** Hard error — prevents Copilot CLI from launching with an
insufficient Node.js version.

**Location:** Called in the `"copilot"` arm of `ensure_tool_ready()`, before
`ensure_copilot_home_staged()`.

```
ensure_tool_ready("copilot")
├── non-interactive guard (CI skip)
├── check_required_tools()
├── ensure_framework_installed()
├── ensure_recipe_runner_up_to_date()
├── check_node_minimum_version(24)  ← NEW: hard error on Node < v24
└── ensure_copilot_home_staged()
```

If `check_node_minimum_version` returns an error, bootstrap prints the error
(including the platform-specific install hint) and returns `Err`, preventing
the Copilot CLI from launching into a guaranteed failure.

### `amplihack install` (install/mod.rs)

**Behavior:** Warning — prints a prominent message but does not fail the
install, since `amplihack install` configures many tools beyond just Copilot.

**Location:** Called at the Copilot plugin registration step, before
`register_copilot_plugin()`.

```
local_install(repo_root)
├── ...phases 1-5...
├── 🐙 Configuring GitHub Copilot CLI plugin:
│   ├── check_node_minimum_version(24)  ← NEW: warning on Node < v24
│   └── register_copilot_plugin()
├── ...phases 6-8...
```

If the version check fails, the warning is printed with the `⚠️` prefix and
includes the detected version and upgrade instructions. The install continues
and the Copilot plugin is still registered (it may work after a future Node.js
upgrade without re-running install).

## Configuration

The minimum version (`24`) is passed as a parameter at each call site, not
hardcoded in `amplihack_utils`. This allows call sites to adjust the threshold
independently if requirements change.

No environment variables, config files, or feature flags control this behavior.
The check cannot be bypassed except by upgrading Node.js.

## Security

- **No new subprocess spawning.** `check_node_minimum_version` reuses the
  existing `check_tool("node")` infrastructure in `prerequisites.rs`, which
  runs `node --version` via `ProcessManager` with the standard 5-second timeout.
- **`parse_node_major_version` is a pure function** — `fn(&str) -> Option<u32>`.
  No shell expansion, no `eval`, no environment interaction.
- **Fail-open on unparseable versions** — only blocks on *known-insufficient*
  versions. This prevents the check from becoming a denial-of-service vector
  in unusual environments.
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
