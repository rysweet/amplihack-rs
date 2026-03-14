# How to Uninstall amplihack

`amplihack uninstall` removes everything the installer placed on disk: staged framework files, deployed binaries, runtime directories, and hook registrations in `~/.claude/settings.json`.

## Run the Uninstall Command

```sh
amplihack uninstall
```

The command reads `~/.amplihack/.claude/install/amplihack-manifest.json` and removes every path listed in it. It then removes hook registrations from `~/.claude/settings.json`.

## What Gets Removed

Uninstall proceeds in four phases:

| Phase | What is removed |
|-------|----------------|
| **Phase 1 — Tracked files** | Every file listed in `manifest.files[]` under `~/.amplihack/.claude/` |
| **Phase 2 — Tracked directories** | Every directory listed in `manifest.dirs[]` under `~/.amplihack/.claude/`, deepest first |
| **Phase 3 — Deployed binaries** | `amplihack` and `amplihack-hooks` from `~/.local/bin/` if they appear in `manifest.binaries[]` |
| **Phase 4 — Hook registrations** | All `amplihack-hooks` and `tools/amplihack/` entries from `~/.claude/settings.json` |

After the command completes, the terminal prints a summary:

```
✓ Removed 47 files
✓ Removed 12 directories
✓ Removed 2 binaries from ~/.local/bin
✓ Removed 7 hook registrations from settings.json
amplihack uninstalled successfully.
```

## What Is NOT Removed

| Item | Why it stays |
|------|-------------|
| `~/.claude/settings.json` itself | Other tools may use it |
| `~/.claude/settings.json.backup.*` | Kept as safety snapshots |
| XPIA hook registrations | XPIA is an independent tool; its entries are preserved |
| Your own `.claude/` files | Only amplihack-owned paths from the manifest are touched |
| Python `amplihack` package | Installed via pip, not by this CLI |

## If the Manifest Is Missing

If `amplihack-manifest.json` does not exist (for example after a partial install), the uninstall command prints a warning and falls back to removing the well-known hardcoded directories:

```
⚠️  Manifest not found at ~/.amplihack/.claude/install/amplihack-manifest.json
    Falling back to hardcoded directory list.
```

The hardcoded fallback removes:
- `~/.amplihack/.claude/agents/amplihack`
- `~/.amplihack/.claude/commands/amplihack`
- `~/.amplihack/.claude/tools/amplihack`

If these directories do not exist, the command exits cleanly with no error.

## Re-installing After Uninstall

```sh
# Clean reinstall from GitHub
amplihack install

# Clean reinstall from local checkout
amplihack install --local ~/src/amplihack
```

Hook registrations are fully restored by a subsequent install.

### Why Reinstall Works After Uninstall

`amplihack uninstall` removes only the `~/.local/bin` copies (Phase 3).
`find_hooks_binary()` reaches PATH at Step 3 — before checking `~/.local/bin` at Step 4 — so
binaries that survived uninstall (e.g. a system-wide tarball install) are found without requiring a rebuild.

| Install scenario | Lookup step that resolves it |
|-----------------|------------------------------|
| Tarball to `/usr/local/bin` | Step 2 (sibling-of-exe) or Step 3 (PATH) |
| `cargo build --release` + run from `target/release/` | Step 2 (sibling-of-exe) |
| `cargo install` | Step 5 (`~/.cargo/bin`) |

If the binary is not found after uninstall, rebuild it before reinstalling:

```sh
cargo build --release --bin amplihack-hooks
```

## See Also

- [Install amplihack for the First Time](./first-install.md)
- [Install Manifest reference](../reference/install-manifest.md) — manifest schema details
- [amplihack install reference](../reference/install-command.md) — install and uninstall flags
