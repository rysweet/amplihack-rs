# Reference: Copilot CLI Installation Implementation

Technical reference for maintainers of the Copilot CLI installation system.

## Architecture

### Installation Flow

```
┌─────────────────────────────────────┐
│ launch_copilot()                    │
│                                     │
│ 1. Check if already installed      │
│    shutil.which("copilot")         │
│                                     │
│ 2. Install if needed               │
│    install_copilot()               │
│    - Runs npm install -g           │
│    - Returns True/False            │
│                                     │
│ 3. Report status                   │
│    - Trust installer return value  │
│    - Print success/failure         │
│    - Exit with appropriate code    │
│                                     │
│ 4. Launch CLI                      │
│    subprocess.run(["copilot"])     │
└─────────────────────────────────────┘
```

### Current binary contract

The current runtime launches the `copilot` CLI from `@github/copilot`. Older
references to `github-copilot-cli` below are historical and should not be read
as the current install/launch contract.

### Module Structure

**File**: `amplihack/launcher/__init__.py`

```rust
def launch_copilot() -> None:
    """Launch GitHub Copilot CLI, installing if needed."""

def install_copilot() -> bool:
    """Install GitHub Copilot CLI via npm.

    Returns:
        True if installation succeeded, False otherwise
    """
```

## Implementation Details

### Installation Check

**Function**: `shutil.which("copilot")`

**Purpose**: Check if Copilot CLI binary is in PATH

**Behavior**:

- Returns full path if found: `/home/user/.local/bin/copilot`
- Returns `None` if not found
- Searches current process's PATH environment variable
- Does not search outside PATH

**Limitations**:

- Only sees PATH at process start time
- Does not detect binaries added after process starts
- Subprocess environment may differ from parent

### Installation Process

**Function**: `install_copilot()`

**Implementation**:

```rust
def install_copilot() -> bool:
    """Install GitHub Copilot CLI via npm."""
    try:
        result = subprocess.run(
            ["npm", "install", "-g", "@github/copilot"],
            check=False,  # Don't raise on failure
            capture_output=True,
            text=True
        )
        return result.returncode == 0
    except FileNotFoundError:
        # npm not found
        return False
    except Exception as e:
        print(f"Installation error: {e}")
        return False
```

**Validation**:

- Checks npm exit code (0 = success)
- Does NOT re-verify with `shutil.which()`
- Trusts npm's success indication

**Why trust npm exit code?**

- npm validates installation internally
- Exit code 0 guarantees success
- Binary is written before npm exits
- Avoids PATH propagation race condition

### Status Reporting

**Success Path**:

```rust
if not shutil.which("github-copilot-cli"):
    success = install_copilot()
    if not success:
        print("Failed to install Copilot CLI")
        sys.exit(1)
    print("Successfully installed Copilot CLI")
```

**Failure Path**:

```rust
# npm install failed
if not success:
    print("Failed to install Copilot CLI")
    sys.exit(1)
```

## Bug History

### Original Bug (v0.3.1)

**Code**:

```rust
if not shutil.which("github-copilot-cli"):
    success = install_copilot()

    # Redundant verification (BUG)
    if not shutil.which("github-copilot-cli"):
        print("Failed to install Copilot CLI")
        sys.exit(1)
```

**Issue**: Redundant `shutil.which()` check after installation

**Root Cause**: PATH propagation timing

**Symptoms**: False negative when installation succeeded

### The Fix (v0.3.2)

**Change**: Remove redundant verification

**Code**:

```rust
if not shutil.which("github-copilot-cli"):
    success = install_copilot()
    if not success:
        print("Failed to install Copilot CLI")
        sys.exit(1)
    print("Successfully installed Copilot CLI")
```

**Result**: Accurate status reporting

## PATH Propagation

### The Problem

**Scenario**: Install binary, check PATH immediately

**What happens**:

1. npm installs binary to `~/.local/bin/github-copilot-cli`
2. Binary exists on disk
3. Shell updates PATH (asynchronously)
4. Current process hasn't seen PATH update yet
5. `shutil.which()` can't find binary

**Timeline**:

```
T+0ms:  npm install starts
T+100ms: Binary written to disk
T+110ms: npm exits with code 0
T+120ms: Shell updates PATH
T+130ms: shutil.which() checks PATH (binary not visible yet)
T+200ms: PATH propagates to current process
```

**Solution**: Trust installer exit code, don't re-check PATH

### Platform Differences

**Linux/macOS**:

- PATH updates propagate to child processes
- Parent process sees stale PATH
- `shutil.which()` searches parent's PATH

**Windows**:

- Similar behavior with different timing
- Registry updates may be involved
- Environment blocks cached per process

## Testing Strategy

### Unit Tests

**Test**: Installation triggers when needed

```rust
def test_launch_copilot_installs_when_missing(monkeypatch):
    """Verify installation happens when CLI not found."""
    with patch('shutil.which', return_value=None), \
         patch('amplihack.launcher.install_copilot', return_value=True), \
         patch('subprocess.run'):
        launch_copilot()
        # Should succeed without raising
```

**Test**: Failure reported accurately

```rust
def test_launch_copilot_reports_install_failure(monkeypatch):
    """Verify failure message when installation fails."""
    with patch('shutil.which', return_value=None), \
         patch('amplihack.launcher.install_copilot', return_value=False):
        with pytest.raises(SystemExit) as exc:
            launch_copilot()
        assert exc.value.code == 1
```

**Test**: No installation when already present

```rust
def test_launch_copilot_skips_install_when_present(monkeypatch):
    """Verify no installation when CLI already exists."""
    install_mock = Mock()
    with patch('shutil.which', return_value='/usr/bin/github-copilot-cli'), \
         patch('amplihack.launcher.install_copilot', install_mock), \
         patch('subprocess.run'):
        launch_copilot()
        install_mock.assert_not_called()
```

### Integration Tests

**Test**: Fresh installation

```bash
# Remove Copilot CLI
npm uninstall -g github-copilot-cli

# Verify not installed
! which github-copilot-cli

# Run amplihack
amplihack copilot

# Verify success
test $? -eq 0
which github-copilot-cli
```

**Test**: Existing installation

```bash
# Ensure installed
npm install -g github-copilot-cli

# Run amplihack (should skip install)
amplihack copilot

# Verify success
test $? -eq 0
```

## Error Handling

### npm Not Found

```rust
try:
    result = subprocess.run(["npm", ...])
except FileNotFoundError:
    return False  # npm not installed
```

**User sees**: "Failed to install Copilot CLI"

**Solution**: Install npm: `sudo apt install npm`

### npm Install Failure

**Causes**:

- Network issues
- Permissions problems
- Disk space exhausted
- Package not found

**Detection**: `result.returncode != 0`

**User sees**: "Failed to install Copilot CLI"

**Solution**: Check npm logs: `npm install -g github-copilot-cli`

### Permission Denied

**Cause**: Global npm install requires write access

**Detection**: npm exit code, stderr contains "EACCES"

**Solution**: Use `sudo` or configure npm prefix:

```bash
npm config set prefix ~/.local
export PATH=~/.local/bin:$PATH
```

## Design Decisions

### Why Trust npm Exit Code?

**Options considered**:

1. Trust npm exit code (chosen)
2. Re-verify with `shutil.which()`
3. Check binary exists on disk
4. Test execution with `--version`

**Decision**: Trust npm exit code

**Rationale**:

- npm validates internally before exiting
- Avoids PATH propagation timing issues
- Simplest implementation
- npm exit codes are reliable (industry standard)

### Why Not Test Execution?

**Alternative**: Run `github-copilot-cli --version` to verify

**Rejected because**:

- Adds complexity (parse version output)
- Same PATH propagation issue
- Unnecessary overhead
- npm validation is sufficient

### Why No Retry Logic?

**Alternative**: Retry if verification fails

**Rejected because**:

- Masks real installation failures
- Delays feedback to user
- npm should succeed or fail definitively
- Retries won't fix PATH timing (not a transient error)

## Maintenance Notes

### When to Modify

**Change installer if**:

- npm package name changes
- Installation method changes (not npm)
- Different global vs. local install needed

**Don't change if**:

- Seeing "PATH not propagated" issues (by design)
- Want to verify installation (trust npm)
- Need to support alternative install methods (use separate function)

### Testing Checklist

Before releasing changes:

- [ ] Unit tests pass
- [ ] Integration test on fresh system
- [ ] Test with Copilot already installed
- [ ] Test with npm not installed
- [ ] Test with permission denied errors
- [ ] Test on Linux, macOS, Windows
- [ ] Exit codes correct (0 success, 1 failure)
- [ ] Messages clear and accurate

### Code Review Focus

**Check these during review**:

1. **No redundant verification**: Don't re-check after installation
2. **Trust installer**: Use return value, not PATH lookup
3. **Exit codes correct**: 0 for success, 1 for failure
4. **Messages accurate**: Match actual result
5. **Error handling**: Catch npm not found, install failures

## Related Code

**Files**:

- `amplihack/launcher/__init__.py` - Main implementation
- `tests/test_launcher.sh` - Unit tests
- `scripts/install.sh` - Installation script

**Dependencies**:

- `shutil.which()` - PATH lookup
- `subprocess.run()` - Execute npm, launch CLI
- `sys.exit()` - Exit with status code

## Node.js Runtime Prerequisite

Before any npm-based installation can proceed, the Rust CLI ensures a
compatible Node.js runtime is available. `check_node_minimum_version()`
probes the system and returns one of:

- `Ok(())` — system node is ≥ 18, proceed normally
- `Err(NotFound)` — `node` binary is absent from PATH
- `Err(TooOld)` — node exists but version < 18
- `Err(VersionUndetectable)` — node exists but version output is unparseable

Any `Err` variant triggers `ensure_node_for_copilot()`, which downloads
an official Node.js binary distribution to `~/.amplihack/runtimes/` and
prepends its `bin/` directory to `PATH`. Extraction uses atomic staging
(`.extracting` temp directory → rename) to prevent broken partial installs.

See [Node.js Runtime Auto-Install](../concepts/node-runtime-auto-install.md)
for full details and [`NodeVersionError` Reference](../reference/node-version-error.md)
for the error enum API.

## Rust CLI: Two-Phase Installation (Current)

The Rust CLI (`bootstrap.rs`) replaces the Python launcher and uses a
two-phase installation strategy to work around an npm 9.x bug where
platform-mismatched optional dependencies cause npm to hang indefinitely
during the reify phase.

### Architecture (Rust)

```
┌──────────────────────────────────────────────────────────────┐
│ ensure_tool_available("copilot")                             │
│                                                              │
│ 1. BinaryFinder::find("copilot")                            │
│    └─ Found? → maybe_upgrade_tool() → return BinaryInfo     │
│    └─ Not found? → install_tool("copilot")                  │
│                                                              │
│ 2. install_tool("copilot")                                   │
│    └─ npm_package_for_install("copilot") → "@github/copilot"│
│    └─ install_npm_package("copilot", "@github/copilot")     │
│                                                              │
│ 3. install_npm_package()                                     │
│    ├─ Phase 1: npm install --omit=optional                   │
│    │   (base package without platform-specific deps)         │
│    ├─ Phase 2: npm install @github/copilot-{os}-{arch}       │
│    │   (platform-specific native binary only)                │
│    │   └─ Failure is non-fatal (JS fallback exists)          │
│    └─ Retry with cleanup on first failure                    │
│                                                              │
│ 4. BinaryFinder::find("copilot") → BinaryInfo               │
└──────────────────────────────────────────────────────────────┘
```

### Phase 1: Base Package (`--omit=optional`)

```rust
// run_npm_install() will add --omit=optional to skip platform-mismatched deps
npm install -g --prefix ~/.npm-global @github/copilot --ignore-scripts --omit=optional
```

The `--omit=optional` flag tells npm to skip all optional dependencies.
This avoids the npm 9.x reify hang caused by attempting to download
platform-mismatched native binaries (e.g., `@github/copilot-darwin-arm64`
on Linux).

### Phase 2: Platform-Specific Binary

```rust
// copilot_platform_package() will determine the correct package
// Based on std::env::consts::OS and std::env::consts::ARCH
npm install -g --prefix ~/.npm-global @github/copilot-linux-x64 --ignore-scripts
```

After the base package is installed, `install_npm_package()` will install only
the native binary package for the current platform. If this fails, a warning
is logged but installation continues — the `@github/copilot` package includes
a JavaScript fallback that may work without the native binary on sufficiently
recent Node.js versions (verify against the actual package requirements).

### Platform Detection (`copilot_platform_package()`)

| `std::env::consts::OS` | `std::env::consts::ARCH` | Package                        |
|------------------------|-------------------------|--------------------------------|
| `linux`                | `x86_64`                | `@github/copilot-linux-x64`   |
| `macos`                | `aarch64`               | `@github/copilot-darwin-arm64` |
| `macos`                | `x86_64`                | `@github/copilot-darwin-x64`  |
| `windows`              | `x86_64`                | `@github/copilot-win32-x64`   |
| Other                  | Other                   | `None` (skip phase 2)          |

### Error Messages

Installation errors now include structured output with:
- Package name and failure reason
- Copy-pasteable manual fix commands
- Cleanup steps for stale npm state

See [Actionable npm Installation Error Messages](../features/npm-install-error-messages.md)
for the complete error message catalog.

### Why `--omit=optional` Instead of `--os`/`--cpu`?

The npm `--os` and `--cpu` flags were evaluated as a potential fix during
issue #585 triage but rejected. These flags are documented as platform
override hints, but npm 9.x
ignores them during optional dependency resolution. The `--omit=optional`
approach is correct because:

1. It completely skips optional dependency resolution (no reify hang)
2. It is supported in npm 8.x+ (safe for npm 9.2.0)
3. The platform binary is installed separately as a targeted, single-package
   install that never triggers the multi-platform reify bug
4. Failure of the platform binary install is non-fatal — JS fallback exists

## Future Improvements

### Potential Enhancements

1. **Verbose mode**: Show npm output on failure
2. **Offline install**: Support installing from cache
3. **Version check**: Verify minimum version requirements
4. ~~**Auto-upgrade**: Detect outdated installation~~ — Implemented in `maybe_upgrade_tool()`
5. **Alternative installers**: Support pip, brew, etc.

### Not Recommended

1. **Retry verification**: Masks real issues
2. ~~**PATH manipulation**: Fragile, platform-specific~~ — Implemented in `prepend_path()` + `persist_path_hint()`
3. **Binary download**: npm handles this better
4. **Version pinning**: Users may want latest
5. **`--os`/`--cpu` npm flags**: Broken in npm 9.x, do not use

---

**Audience**: Maintainers and contributors
**Scope**: Implementation details
**Last Updated**: 2026-06-02
**Version**: Rust CLI (bootstrap.rs)

## See Also

- [Node.js Runtime Auto-Install](../concepts/node-runtime-auto-install.md) — automatic Node.js provisioning
- [`NodeVersionError` Reference](../reference/node-version-error.md) — error enum API
- [Bug Fix #679](../bugfixes/bugfix-679-node-auto-install-quality-fixes.md) — quality audit fixes for auto-install
