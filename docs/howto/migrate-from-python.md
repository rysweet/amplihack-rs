# Migrate from amplihack (Python) to amplihack-rs

This guide covers switching from the Python `amplihack` package (installed via `uvx` or `pip`) to the native Rust binary `amplihack-rs`.

## Prerequisites

- An existing amplihack Python installation (`amplihack version` shows `0.6.x`)
- Rust toolchain (for building from source) **or** access to GitHub releases

## Step 1: Install amplihack-rs

### Option A: From GitHub releases (recommended)

```bash
# Download the latest release binary
amplihack update
```

If you don't have the Rust binary yet:

```bash
# Download and install from releases
curl -fsSL https://github.com/rysweet/amplihack-rs/releases/latest/download/amplihack-$(uname -s)-$(uname -m) -o ~/.local/bin/amplihack
chmod +x ~/.local/bin/amplihack
```

### Option B: From source

```bash
cargo install --git https://github.com/rysweet/amplihack-rs amplihack --locked
```

### Option C: From a local clone

```bash
cd ~/src/amplihack-rs
cargo build --release
cp target/release/amplihack ~/.local/bin/amplihack
cp target/release/amplihack-hooks ~/.local/bin/amplihack-hooks
cp target/release/amplihack-asset-resolver ~/.local/bin/amplihack-asset-resolver
chmod +x ~/.local/bin/amplihack*
```

## Step 2: Verify the Rust binary is on PATH

The Rust binary must appear **before** the Python `uvx` binary on your `PATH`:

```bash
type -a amplihack
```

Expected output:
```
amplihack is /home/you/.local/bin/amplihack      # ← Rust (should be first)
amplihack is /home/you/.cache/uv/.../amplihack   # ← Python (should be second or removed)
```

If the Python version appears first, either:

1. **Add `~/.local/bin` to the front of PATH** in your shell profile:
   ```bash
   # Add to ~/.bashrc or ~/.zshrc:
   export PATH="$HOME/.local/bin:$PATH"
   ```

2. **Or remove the Python version**:
   ```bash
   uv tool uninstall amplihack
   ```

## Step 3: Run install

```bash
amplihack install
```

This stages hooks, agents, skills, workflows, and context files to `~/.amplihack/.claude/` and wires `~/.claude/settings.json`.

## Step 4: Verify

```bash
amplihack doctor
```

Expected output:
```
✓ amplihack hooks installed
✓ settings.json is valid JSON
✓ recipe-runner-rs recipe-runner 0.3.x
✓ tmux 3.x
✓ amplihack v0.7.x

All checks passed.
```

## Step 5: Test key commands

```bash
# Version
amplihack version          # Should show "amplihack-rs 0.7.x"

# Recipe runner
amplihack recipe list      # Should list 17+ recipes

# Launch copilot
amplihack copilot --help   # Should show copilot options

# Update
amplihack update           # Should check for latest release
```

## Command mapping

All Python CLI commands have Rust equivalents:

| Command | Python | Rust | Notes |
|---------|--------|------|-------|
| `amplihack install` | ✅ | ✅ | Rust clones from git, stages assets |
| `amplihack copilot` | ✅ | ✅ | Same flags, native launcher |
| `amplihack recipe run` | ✅ | ✅ | Both use `recipe-runner-rs` binary |
| `amplihack recipe list` | ✅ | ✅ | Same YAML discovery |
| `amplihack plugin` | ✅ | ✅ | install/uninstall/link/verify |
| `amplihack mode` | ✅ | ✅ | detect/to-plugin/to-local |
| `amplihack memory` | ✅ | ✅ | tree/export/import/clean |
| `amplihack update` | ✅ | ✅ | Checks GitHub releases |
| `amplihack doctor` | ✅ | ✅ | System health checks |
| `amplihack fleet` | ✅ | ✅ | Native Rust fleet runtime |
| `amplihack new` | ✅ | ✅ | Agent generator |
| `amplihack index-code` | ❌ | ✅ | New: native code graph |
| `amplihack query-code` | ❌ | ✅ | New: code graph queries |
| `amplihack completions` | ❌ | ✅ | New: shell completions |

## What changes

- **Hooks are native binaries** — no Python interpreter needed at hook time
- **Startup is faster** — no Python/pip dependency resolution
- **Self-update works** — `amplihack update` downloads release binaries directly
- **Code intelligence** — `index-code` and `query-code` are new Rust-native features

## What stays the same

- All agent/skill/workflow content is identical (same amplifier-bundle)
- Recipe execution uses the same `recipe-runner-rs` binary
- `~/.amplihack/.claude/` directory structure is unchanged
- `~/.claude/settings.json` format is unchanged

## Troubleshooting

### Python version still resolving first
```bash
type -a amplihack    # Check which binary is first
which amplihack      # Should be ~/.local/bin/amplihack or ~/.cargo/bin/amplihack
```
Fix: ensure `~/.local/bin` is before `~/.cache/uv/...` in your `$PATH`.

### `mode detect` says "no .claude installation found"
This checks for a Claude Code-specific installation marker. If you use Copilot as your primary agent, this is expected — the mode command targets Claude Code installations.

### Memory commands show WAL corruption
```bash
amplihack memory clean    # Resets the memory database
```

### Missing recipe-runner-rs
```bash
amplihack doctor    # Will show if recipe-runner-rs is missing
# Install it:
cargo install --git https://github.com/rysweet/amplihack-recipe-runner recipe-runner --locked
```
