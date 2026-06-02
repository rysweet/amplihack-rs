# Bug Fix #679 — `amplihack install` Does Not Ensure Its Runtime Dependencies

> **Issue:** [#679](https://github.com/rysweet/amplihack-rs/issues/679)
> **PR:** [#680](https://github.com/rysweet/amplihack-rs/pull/680) (original), quality audit fixes applied on top

---

## Summary

`amplihack install` and `amplihack copilot` silently skip Node.js auto-install
when Node.js is completely missing from the system. The auto-install feature
(introduced in PR #680) works when Node.js is present but too old, but fails
to trigger when Node.js is not installed at all — the exact scenario it exists
to fix.

Additionally, the extraction step writes directly to the final install
directory, so an interrupted download (disk full, SIGKILL) leaves a broken
partial install that subsequent runs mistake for a successful installation.

## Root Causes

### Issue 1 (Critical): `check_node_minimum_version()` returns `Ok(())` when node is absent

When `node --version` fails because the binary doesn't exist,
`check_node_minimum_version()` returns `Ok(())` — signaling "node is
fine." Callers use `.is_ok()` to decide whether to skip auto-install, so
a missing node is treated as a sufficient node.

**Before fix:**
```
node not found → result.found = false → return Ok(()) → .is_ok() = true → skip install
```

**After fix:**
```
node not found → result.found = false → return Err(NotFound) → .is_ok() = false → auto-install
```

The fix adds a `NodeVersionError::NotFound` variant and returns it when
`result.found` is `false`. This is distinct from `VersionUndetectable`
(node exists but version output is unparseable).

### Issue 2 (High): Windows platform triple enables broken download path

`node_platform_triple()` returns `"win"` for Windows, but the extraction
code uses `tar -xJf` which only works on `.tar.xz` archives. Node.js
distributes Windows builds as `.zip` files. A Windows user would download
a `.tar.xz` URL that doesn't exist on nodejs.org and get a confusing 404.

The fix returns `None` on Windows, causing `ensure_node_for_copilot()` to
bail with a clear "auto-install not supported on this platform" message.

### Issue 3 (Medium): Non-atomic extraction leaves broken installs

The original code extracts the Node.js tarball directly into the final
install directory (`~/.amplihack/runtimes/node-v24.1.0-linux-x64/`). If
extraction is interrupted, a partial directory with `bin/node` missing
(or corrupted) is left behind. The next run finds the directory, sees it
exists, and assumes installation succeeded.

The fix extracts into a staging directory (`{dir_name}.extracting`),
verifies `bin/node` exists, and atomically renames to the final path.
Partial extractions are cleaned up on the next run.

### Issue 4 (Medium): `install/mod.rs` inherits Issue 1

`install/mod.rs` also calls `check_node_minimum_version()` and receives
the same incorrect `Ok(())` when node is missing. Fixed automatically by
the `NotFound` variant change — no code changes needed in `install/mod.rs`.

## Files Changed

| File | Change |
|------|--------|
| `crates/amplihack-utils/src/prerequisites.rs` | Added `NotFound` variant to `NodeVersionError`; return `Err(NotFound)` when `result.found` is false; return `None` from `node_platform_triple()` on Windows |
| `crates/amplihack-cli/src/bootstrap.rs` | Simplified `let ext = "tar.xz"` (removed dead Windows branch); atomic temp-dir extraction with `--strip-components=1`, `bin/node` verification, and `fs::rename()` |
| `crates/amplihack-utils/src/tests/prerequisites_tests.rs` | Updated platform triple assertion to `["linux", "darwin"]`; added `node_version_error_not_installed_displays_actionable_message` and `node_version_error_not_found_is_distinct` tests |

## Verification

After the fix:

```
# Node.js not installed — auto-install triggers
$ amplihack copilot
📦 Node.js is not installed. Downloading Node.js v24.1.0...
✓ Installed Node.js v24.1.0 to ~/.amplihack/runtimes/node-v24.1.0-linux-x64
🚀 Launching Copilot CLI...

# Interrupted extraction — clean retry
$ amplihack copilot   # (after killing during extraction)
⚠️  Cleaning up incomplete extraction: node-v24.1.0-linux-x64.extracting
📦 Node.js is not installed. Downloading Node.js v24.1.0...
✓ Installed Node.js v24.1.0 to ~/.amplihack/runtimes/node-v24.1.0-linux-x64

# Windows — clear error
$ amplihack copilot   # (on Windows)
✗ Automatic Node.js installation is not supported on this platform.
  Install Node.js manually: https://nodejs.org/
```

## Related

- [Node.js Runtime Auto-Install](../concepts/node-runtime-auto-install.md) —
  concept documentation for the auto-install feature
- [`NodeVersionError` Reference](../reference/node-version-error.md) —
  API reference for the error enum
- [Bootstrap Parity](../concepts/bootstrap-parity.md) — the overall
  install sequence
- [Copilot Installation Implementation](../reference/copilot-installation-implementation.md) —
  npm-based tool installation reference
