# Windows Build Target — Reference

## Overview

amplihack-rs ships a pre-built binary for `x86_64-pc-windows-msvc` alongside the existing Linux and macOS targets. The Windows binary is produced by the same CI/CD pipeline that builds the other release artifacts, using a `windows-latest` GitHub Actions runner with native compilation (no cross-compilation toolchain required).

## Supported Target

| Target triple | Runner | Compilation |
|---------------|--------|-------------|
| `x86_64-pc-windows-msvc` | `windows-latest` | Native (MSVC) |

The following targets were already supported before the Windows addition:

| Target triple | Runner | Compilation |
|---------------|--------|-------------|
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | Native |
| `aarch64-unknown-linux-gnu` | `ubuntu-latest` | Cross (GNU toolchain: `gcc-aarch64-linux-gnu`) |
| `x86_64-apple-darwin` | `macos-latest` | Native |
| `aarch64-apple-darwin` | `macos-latest` | Native |

## Release Artifacts

Each GitHub release includes the following artifacts for the Windows target:

| File | Contents |
|------|----------|
| `amplihack-x86_64-pc-windows-msvc.tar.gz` | `amplihack.exe` and `amplihack-hooks.exe` |
| `amplihack-x86_64-pc-windows-msvc.tar.gz.sha256` | SHA-256 checksum of the tarball |

The checksum is computed on the build runner immediately after packaging, before artifact upload, preserving chain of custody.

## Installation on Windows

### From a Release

```powershell
# Download the latest release
$version = "0.9.1"
$base = "https://github.com/rysweet/amplihack-rs/releases/download/v$version"
Invoke-WebRequest "$base/amplihack-x86_64-pc-windows-msvc.tar.gz" -OutFile amplihack.tar.gz

# Verify checksum
$expected = (Invoke-WebRequest "$base/amplihack-x86_64-pc-windows-msvc.tar.gz.sha256").Content.Trim().Split()[0]
$actual = (Get-FileHash amplihack.tar.gz -Algorithm SHA256).Hash.ToLower()
if ($expected -ne $actual) { throw "Checksum mismatch" }

# Extract to a directory on PATH (requires tar, available on Windows 10 1803+)
tar xzf amplihack.tar.gz -C "$env:USERPROFILE\.local\bin"
```

### From Source

```powershell
# Requires Rust toolchain (rustup.rs) with the MSVC target
rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
```

Binaries are written to `target\x86_64-pc-windows-msvc\release\amplihack.exe` and `amplihack-hooks.exe`.

## Platform Behaviour

### Subcommands Available on Windows

All subcommands are available on Windows with the following notes:

| Subcommand | Windows behaviour |
|------------|------------------|
| `install` | Deploys binaries to `%USERPROFILE%\.local\bin`; reads `%USERPROFILE%\.claude\settings.json` |
| `doctor` | All 7 checks run. `tmux` check will fail if tmux is not installed (expected on bare Windows). |
| `completions` | All four shells supported (`bash`, `zsh`, `fish`, `powershell`). |
| `launch` | Spawns `claude.exe` if on PATH |
| `run` / `list` / `validate` | No platform-specific differences |

### Process Termination

On Unix, amplihack's `ManagedChild` sends `SIGTERM` before `SIGKILL` to allow graceful shutdown. On Windows, `SIGTERM` does not exist; `ManagedChild::terminate()` calls `Child::kill()` directly. This is equivalent to `TerminateProcess` and is the correct Windows idiom for the same intent.

### Settings and Home Directory

On Windows, `settings.json` is resolved via the `HOME` environment variable (if set) or `%USERPROFILE%`. The resolved path is `<home>\.claude\settings.json`, matching the Unix convention `~/.claude/settings.json`.

## Known Limitations

### tmux

tmux does not have a native Windows port. `amplihack doctor` will report `✗ tmux installed` on Windows unless a Unix compatibility layer (WSL2, MSYS2, Cygwin) provides `tmux` on PATH. This does not prevent other amplihack features from working.

### kuzu

The `kuzu` dependency (graph memory backend) is assumed to be Windows-compatible at the source level. If the `kuzu` build scripts have undiscovered Unix-only assumptions that surface in future versions, the affected functionality can be compiled out with `--no-default-features`.

### ANSI Colours

amplihack always emits ANSI escape codes. Windows Terminal and VS Code's integrated terminal render them correctly. Older `cmd.exe` environments may display raw escape bytes. This is a known v1 limitation; TTY detection and Win32 console API support are out of scope.

## CI Pipeline Details

The Windows build runs as part of two workflows:

### `ci.yml` — Pull Request Checks

The cross-compile matrix includes `x86_64-pc-windows-msvc` with `runner: windows-latest` and `cross: false`. The build step is:

```yaml
- name: Build
  run: cargo build --release --target ${{ matrix.target }}
```

Artifact upload includes both `amplihack.exe` and `amplihack` (for non-Windows targets) via a multi-extension path pattern.

### `release.yml` — Release Builds

The release matrix includes `x86_64-pc-windows-msvc`. The Package step uses `shell: bash` (provided by Git for Windows on `windows-latest`) to keep the packaging script consistent across all runners. All targets, including Windows, are packaged as `.tar.gz`:

```yaml
- name: Package
  shell: bash
  run: |
    mkdir -p dist
    cp "target/${{ matrix.target }}/release/amplihack" dist/ 2>/dev/null || true
    cp "target/${{ matrix.target }}/release/amplihack.exe" dist/ 2>/dev/null || true
    cp "target/${{ matrix.target }}/release/amplihack-hooks" dist/ 2>/dev/null || true
    cp "target/${{ matrix.target }}/release/amplihack-hooks.exe" dist/ 2>/dev/null || true
    cd dist && tar czf "../amplihack-${{ matrix.target }}.tar.gz" *
    cd .. && sha256sum "amplihack-${{ matrix.target }}.tar.gz" \
      > "amplihack-${{ matrix.target }}.tar.gz.sha256"
```

## Related

- [How to Install amplihack for the First Time](../howto/first-install.md) — Unix-focused install guide
- [amplihack completions](./completions-command.md) — Generating PowerShell completions on Windows
- [amplihack doctor](./doctor-command.md) — System health checks including Windows-specific behaviour
