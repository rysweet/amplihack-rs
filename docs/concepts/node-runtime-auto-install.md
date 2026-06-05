# Node.js Runtime Auto-Install

## What Is Node.js Runtime Auto-Install?

When `amplihack copilot` is launched and the system's Node.js is missing or too
old, amplihack automatically downloads and installs a compatible Node.js runtime
into `~/.amplihack/runtimes/`. The user normally does not need to install
Node.js manually on supported interactive Linux and macOS hosts.

## Why This Matters

GitHub Copilot CLI requires Node.js v24+. On fresh machines, CI containers, or
hardened servers, Node.js is often absent or stuck at an old system version.
Without auto-install, the user sees a cryptic npm error and has to figure out
how to install Node.js themselves.

Auto-install eliminates this friction: `amplihack copilot` just works.

## How It Works

### Detection

`check_node_minimum_version()` in `amplihack-utils/prerequisites.rs`
probes the system for Node.js:

| Outcome | Error variant | What happens next |
|---------|--------------|-------------------|
| `node` not found on PATH | `NodeVersionError::NotFound` | Auto-install triggered |
| Version < minimum (24) | `NodeVersionError::InsufficientVersion` | Auto-install triggered |
| Version string unparseable | `NodeVersionError::VersionUndetectable` | Auto-install triggered |
| Version ≥ minimum | `Ok(())` | No action needed |

The `NotFound` variant is distinct from `VersionUndetectable` — it means
the `node` binary does not exist at all, not that it exists but printed an
unexpected version string. This distinction matters for error messages:
"Node.js is not installed" vs "could not parse version from node output."

### Download and Extraction

`ensure_node_for_copilot()` in `bootstrap.rs` downloads the official
Node.js binary distribution from `https://nodejs.org/dist/`:

1. Determine the platform triple via `node_platform_triple()`.
2. Download `node-v{VERSION}-{TRIPLE}.tar.xz` to a temp file.
3. Extract into a temporary staging directory (`{dir_name}.extracting`)
   using `tar --strip-components=1 -xJf`.
4. Verify `bin/node` exists inside the staging directory.
5. Atomically rename the staging directory to the final install path.
6. Prepend `{install_dir}/bin` to `PATH` for the current process.

### Atomic Extraction

Extraction uses a two-phase commit to prevent broken partial installs:

```
runtimes/
  node-v24.1.0-linux-x64.extracting/   ← tar extracts here (temp)
  node-v24.1.0-linux-x64/              ← final directory (after rename)
```

If extraction is interrupted (disk full, SIGKILL, power loss), the
`.extracting` directory is left behind. On the next run,
`ensure_node_for_copilot()` cleans it up before retrying. Because
the final `node-v24.1.0-linux-x64/` directory only appears via an
atomic `fs::rename()`, a subsequent run never finds a half-extracted
Node.js and mistakenly thinks it succeeded.

The `--strip-components=1` flag removes the top-level directory from
the tarball (e.g., `node-v24.1.0-linux-x64/bin/node` becomes
`bin/node` inside the staging dir). This is safe because official
Node.js tarballs always contain exactly one top-level directory.

### Supported Platforms

| OS    | Architecture | Platform Triple | Format |
|-------|-------------|-----------------|--------|
| Linux | x86_64      | `linux-x64`     | `.tar.xz` |
| macOS | x86_64      | `darwin-x64`    | `.tar.xz` |
| macOS | aarch64     | `darwin-arm64`  | `.tar.xz` |

Windows is not supported for auto-install because Node.js distributes
Windows builds as `.zip` files, not `.tar.xz`. On Windows,
`node_platform_triple()` returns `None` and the user sees a clear
error message with manual installation instructions.

### Cleanup on Failure

Every error path cleans up after itself:

| Failure | Cleanup |
|---------|---------|
| Download fails | Remove partially-written temp file |
| tar extraction fails | Remove `.extracting` staging directory |
| `bin/node` missing after extraction | Remove staging directory, bail with diagnostic |
| `fs::rename()` fails | Remove staging directory |

## File Layout

After auto-install, `~/.amplihack/runtimes/` contains:

```
~/.amplihack/
  runtimes/
    node-v24.1.0-linux-x64/
      bin/
        node        ← the binary added to PATH
        npm
        npx
      lib/
        node_modules/
      include/
      share/
```

## Relationship to `check_node_minimum_version()`

The prerequisite checker and the auto-installer work together:

```
check_node_minimum_version()
  │
  ├─ Ok(())           → system node is good, skip auto-install
  │
  └─ Err(NotFound)    → no node binary at all
  └─ Err(InsufficientVersion) → node exists but version < 24
  └─ Err(VersionUndetectable) → node exists but version unparseable
       │
       └─ All three Err variants → ensure_node_for_copilot()
            │
            ├─ Downloads & installs node to ~/.amplihack/runtimes/
            └─ Prepends runtimes bin/ to PATH
```

The Copilot launch path uses `.is_ok()` to decide whether system Node.js is
sufficient. Any `Err` variant, including the `NotFound` case where node is
completely absent, triggers the auto-install path on supported interactive
hosts. `amplihack install` uses the same check for a warning only and does not
download Node.js during install.

## Error Messages

### NotFound

```
Node.js is not installed — the `node` binary was not found on PATH.
Install with: sudo apt-get install -y nodejs   (or equivalent for your OS)
```

The install hint is platform-specific, generated by `install_hint()`.

### InsufficientVersion

```
Node.js v16 is installed but v24+ is required.
Upgrade with: sudo apt-get install -y nodejs
```

### Auto-install failure

```
⚠️  Failed to auto-install Node.js runtime: <specific error>
   You can install Node.js manually: https://nodejs.org/
```

## See Also

- [Bootstrap Parity](./bootstrap-parity.md) — the overall install sequence
- [Idempotent Installation](./idempotent-installation.md) — repeated installs are safe
- [`NodeVersionError` Reference](../reference/node-version-error.md) — API reference for the error enum
- [Copilot Installation Implementation](../reference/copilot-installation-implementation.md) — npm-based tool installation
- [Bug Fix #679](../bugfixes/bugfix-679-node-auto-install-quality-fixes.md) — quality audit fixes for this feature
