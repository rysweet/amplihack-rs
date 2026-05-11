# Troubleshooting: npm Hangs During Copilot CLI Installation on WSL/Linux

> [Home](../index.md) > [Troubleshooting](README.md) > npm Hangs on WSL/Linux

## Quick Diagnosis

If `amplihack copilot` hangs indefinitely during installation on WSL or Linux,
the problem is npm's `reify` phase trying to download platform-mismatched
optional dependencies (e.g., `@github/copilot-darwin-arm64` on a Linux host).

This is a known npm bug in versions 9.x and below. Despite the optional
packages declaring correct `os` and `cpu` fields in their `package.json`,
npm attempts to fetch and extract them anyway, then stalls during the reify
rename step.

**Note:** The `npm install --os=linux --cpu=x64` flags do NOT fix this issue.
They are documented as platform-override hints, but npm 9.x ignores them
during optional dependency resolution.

### Symptoms

- `amplihack copilot` prints `📦 Installing copilot via npm package @github/copilot...`
  and then hangs with no further output
- CPU usage is near zero (npm is blocked, not spinning)
- The hang occurs during npm's internal `reify` phase
- Killing the process and retrying produces the same hang
- Other npm packages install fine; only `@github/copilot` is affected
- Running on WSL, Linux, or any non-macOS platform

### Root Cause

The `@github/copilot` package declares optional dependencies on
platform-specific native binaries:

```json
{
  "optionalDependencies": {
    "@github/copilot-darwin-arm64": "...",
    "@github/copilot-darwin-x64": "...",
    "@github/copilot-linux-x64": "...",
    "@github/copilot-win32-x64": "..."
  }
}
```

npm 9.x attempts to download ALL optional dependencies regardless of
platform, then filters during the reify (directory rename) phase. On
WSL/Linux, the download or extraction of macOS/Windows binaries can hang
indefinitely — likely due to a combination of npm's internal locking and
the fact that these packages contain platform-specific native code that
confuses npm's integrity checks.

## Fix (Planned)

Issue [#585](https://github.com/rysweet/amplihack-rs/issues/585) implements a
two-phase installation strategy in `bootstrap.rs` that avoids the npm bug
entirely. Once merged, `amplihack copilot` will apply this automatically:

### Phase 1: Install with `--omit=optional`

```bash
npm install -g --prefix ~/.npm-global @github/copilot --ignore-scripts --omit=optional
```

This installs the base `@github/copilot` package and all non-optional
dependencies. The `--omit=optional` flag tells npm to skip ALL optional
dependencies, avoiding the reify hang completely.

### Phase 2: Install platform-specific binary

```bash
npm install -g --prefix ~/.npm-global @github/copilot-linux-x64 --ignore-scripts
```

After the base package is installed, amplihack installs only the
platform-specific native binary package for the current OS and
architecture. This is a single, small package with no dependency tree
issues.

### Fallback behavior

If the platform-specific binary package fails to install (e.g., on an
unsupported architecture like Linux ARM with musl libc), amplihack logs a
warning but does NOT fail the installation. The `@github/copilot` package
includes a JavaScript fallback (`index.js`) that may work without the native
binary on sufficiently recent Node.js versions. Verify against the actual
`@github/copilot` package requirements before relying on this fallback.

## Manual Workaround

If you need to install manually (e.g., on an older version of amplihack):

```bash
# Step 1: Install base package without optional deps
npm install -g @github/copilot --ignore-scripts --omit=optional

# Step 2: Install your platform's native binary
# Determine your platform package:
#   Linux x64:       @github/copilot-linux-x64
#   macOS ARM:       @github/copilot-darwin-arm64
#   macOS x64:       @github/copilot-darwin-x64
#   Windows x64:     @github/copilot-win32-x64

npm install -g @github/copilot-linux-x64 --ignore-scripts

# Step 3: Verify
copilot --version
```

## Platform Package Mapping

| OS      | Architecture | Package                        |
|---------|-------------|--------------------------------|
| Linux   | x86_64      | `@github/copilot-linux-x64`   |
| macOS   | ARM64 (M1+) | `@github/copilot-darwin-arm64` |
| macOS   | x86_64      | `@github/copilot-darwin-x64`  |
| Windows | x86_64      | `@github/copilot-win32-x64`   |

## Related

- [Copilot Installation False Negative](copilot-installation-false-negative.md) — Different bug where installation succeeds but is reported as failed
- [Bootstrap Parity](../concepts/bootstrap-parity.md) — How the Rust CLI handles first-install setup
- [Copilot Installation Reporting](../features/copilot-installation-reporting.md) — Accurate install status reporting
