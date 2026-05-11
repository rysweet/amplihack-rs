# Feature: Actionable npm Installation Error Messages

Clear, structured error messages when npm package installation fails or times out.

> **Implementation status:** Planned for issue
> [#585](https://github.com/rysweet/amplihack-rs/issues/585). Error message
> formats shown below are target designs — exact strings may differ once
> implementation lands. Update this document after merging.

## Overview

When `amplihack copilot`, `amplihack claude`, or `amplihack codex` fails to
install its backing npm package, the error output will include:

1. **What failed** — the specific package name and failure reason
2. **Why it failed** — timeout, npm exit code, or post-install verification failure
3. **How to fix it** — copy-pasteable manual commands to resolve the issue
4. **How to clean up** — steps to remove stale state before retrying

## Error Scenarios

### Timeout During Installation

If npm does not complete within 5 minutes (300 seconds), amplihack kills the
process and reports:

```
❌ npm install timed out for @github/copilot after 300 seconds.

This usually means npm is stuck downloading or extracting packages.
Common causes:
  • Slow or unstable network connection
  • npm registry is unreachable
  • Platform-mismatched optional dependencies (see below)

To install manually:
  npm install -g @github/copilot --ignore-scripts --omit=optional
  npm install -g @github/copilot-linux-x64 --ignore-scripts

To clean up and retry:
  rm -rf ~/.npm-global/lib/node_modules/@github/copilot
  rm -rf ~/.npm-global/lib/node_modules/@github/.copilot-*
  amplihack copilot
```

### npm Returns Non-Zero Exit Code

```
❌ npm install failed for @github/copilot (exit code 1).

To install manually:
  npm install -g @github/copilot --ignore-scripts --omit=optional
  npm install -g @github/copilot-linux-x64 --ignore-scripts

To clean up stale state and retry:
  rm -rf ~/.npm-global/lib/node_modules/@github/copilot
  rm -rf ~/.npm-global/lib/node_modules/@github/.copilot-*
  amplihack copilot

For verbose npm output, run:
  npm install -g @github/copilot --loglevel verbose
```

### Binary Not Found After Installation

If npm reports success but the expected CLI binary is not on PATH:

```
❌ Failed to locate 'copilot' after installation.

The npm package @github/copilot was installed, but the 'copilot' binary
was not found on PATH. This can happen if:
  • npm's global bin directory is not in your PATH
  • The package installed to an unexpected prefix

Check your npm prefix:
  npm config get prefix

Ensure the bin directory is on PATH:
  export PATH="$HOME/.npm-global/bin:$PATH"

To add permanently, append to your shell profile (~/.bashrc or ~/.zshrc):
  echo 'export PATH="$HOME/.npm-global/bin:$PATH"' >> ~/.bashrc
```

### Platform Binary Install Warning (Non-Fatal)

When the base package installs but the platform-specific native binary fails:

```
⚠️  Could not install platform-specific binary @github/copilot-linux-x64.
    The copilot CLI may use its JavaScript fallback if available.
    To install the native binary manually:
      npm install -g @github/copilot-linux-x64 --ignore-scripts
```

## Error Message Structure

All installation error messages follow a consistent format:

```
❌ <summary of what failed>

<explanation of why>

To install manually:
  <exact commands to run>

To clean up and retry:
  <cleanup commands>
  <retry command>
```

This structure ensures users can:
- Immediately understand the failure
- Copy-paste commands to resolve it
- Clean up any partial state before retrying

## Configuration

No configuration is required. Error messages automatically detect:

- The current platform (for platform-specific package names)
- The npm prefix directory (for cleanup paths)
- The shell profile path (for PATH export suggestions)

## Related

- [Copilot npm Hang on WSL/Linux](../troubleshooting/copilot-npm-hang-wsl-linux.md) — The npm hang bug that motivated improved error messages
- [Copilot Installation Reporting](copilot-installation-reporting.md) — Accurate install status reporting
- [Bootstrap Parity](../concepts/bootstrap-parity.md) — How the Rust CLI handles first-install setup
