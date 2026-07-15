# Binary Resolution Reference

During `amplihack install`, the CLI must locate the `amplihack-hooks` binary before it can write hook command strings to `~/.claude/settings.json`. This page documents the exact 5-step resolution sequence used by `find_hooks_binary()`.

## Resolution Order

`find_hooks_binary()` tries each location in order and returns the first one that resolves to an existing executable file:

```
Step 1: AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH env var
Step 2: Sibling of the running amplihack executable
Step 3: PATH lookup (which amplihack-hooks)
Step 4: ~/.local/bin/amplihack-hooks
Step 5: ~/.cargo/bin/amplihack-hooks
```

If none of the five steps succeeds, the installer fails with an actionable error message listing what was tried.

## Step Details

### Step 1: Environment Variable Override

If `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` is set, its value is used if it points to an existing executable. If the variable is set but the path does not exist, resolution silently continues to Step 2 — the variable is treated as a hint, not a hard requirement.

```sh
# Override for testing
AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH=/tmp/fake-hooks amplihack install
```

This step exists primarily for integration tests, which create a temporary stub binary and set the variable to point to it. If the stub is absent or the variable is stale, resolution falls through to locate the real binary.

### Step 2: Sibling of Current Executable

Looks for `amplihack-hooks` in the same directory as the running `amplihack` binary. This covers the common case of running both binaries from a `cargo build --release` output directory:

```
target/release/
├── amplihack        ← running this
└── amplihack-hooks  ← found here at Step 2
```

### Step 3: PATH Lookup

`find_binary("amplihack-hooks")` walks `$PATH` entries in order and returns the first match. This step runs before `~/.local/bin` because `amplihack uninstall` removes only the `~/.local/bin` copies (Phase 3) — binaries placed in system-wide locations (e.g. `/usr/local/bin`) by a tarball install survive uninstall and must be found on reinstall.

This step also covers the tarball-to-`/usr/local/bin` install pattern where both `amplihack` and `amplihack-hooks` are placed into the same system directory. In that case Step 2 (sibling-of-exe) resolves the binary when running from that directory, but Step 3 provides a reliable fallback when the shell's `PATH` is used instead.

### Step 4: ~/.local/bin

The standard user-local binary directory on Linux systems. This is where `deploy_binaries()` places the binaries after a successful install, so Step 4 covers the "amplihack is already installed, re-running install" case.

### Step 5: ~/.cargo/bin

Where Cargo installs binaries via `cargo install`. Covers the case where a user ran `cargo install --path bins/amplihack-hooks` manually.

## Error Output

If resolution fails, the installer prints a structured error:

```
❌ Could not find amplihack-hooks binary.

Tried:
  1. AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH — not set
  2. /home/alice/src/amplihack-rs-update/target/release/amplihack-hooks — not found
  3. PATH lookup — not found
  4. /home/alice/.local/bin/amplihack-hooks — not found
  5. /home/alice/.cargo/bin/amplihack-hooks — not found

Fix: build the binary first:
  cargo build --release --bin amplihack-hooks
```

## Copilot Registration: Selecting the Deployed Path

`find_hooks_binary()` locates a **source** binary to copy during install. When
installing from a worktree it can legitimately resolve a transient build
artifact such as `<cwd>/target/debug/amplihack-hooks` (via Step 2,
sibling-of-exe). That path is fine as a *copy source*, but it must never be
baked into a persisted hook command string — the `target/` directory is
worktree- and cwd-relative and disappears on `cargo clean` or when the worktree
is removed, producing an exit-127 fail-closed outage at hook execution time.

To avoid this, Copilot hook registration does **not** use the raw
`find_hooks_binary()` result. Instead, `deployed_hooks_binary()` selects the
stable, already-deployed `~/.local/bin/amplihack-hooks` path from the set
returned by `deploy_binaries()`:

```
find_hooks_binary()  →  copy source (may be target/debug)
deploy_binaries()    →  copies to ~/.local/bin/amplihack-hooks
deployed_hooks_binary(&deployed)  →  ~/.local/bin/amplihack-hooks  ← registered
```

Selection keys strictly on the exact `amplihack-hooks` file name, so it is
independent of ordering in the deployed set and can never pick up a stray build
path. An absent entry is a **hard error** — the installer refuses to bake a
guessed path rather than reintroduce the outage.

Note: Claude's `~/.claude/settings.json` continues to use `find_hooks_binary()`
directly; the deployed-path selection is Copilot-specific (see issue #911).

## Using in Tests

The parity test scenario for install sets `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` to a stub executable:

```yaml
# tests/parity/scenarios/tier2-install.yaml
env:
  AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH: "{{tmpdir}}/fake-hooks"
setup: |
  printf '#!/bin/sh\necho "amplihack-hooks stub"\n' > {{tmpdir}}/fake-hooks
  chmod +x {{tmpdir}}/fake-hooks
```

Unit tests in `crates/amplihack-cli/src/commands/install.rs` follow the same pattern and use `home_env_lock()` for serialization when multiple tests manipulate the `HOME`-relative paths.

## See Also

- [amplihack install reference](./install-command.md) — full install command and environment variables
- [First-time install how-to](../howto/first-install.md) — step-by-step install guide
