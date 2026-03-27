# How to Install amplihack for the First Time

This guide walks through running `amplihack install` on a machine that has never had amplihack installed. The command is self-contained: it validates prerequisites, deploys binaries, stages framework assets, and wires all Claude Code hooks in a single invocation.

## Prerequisites

| Requirement | Minimum version | Why |
|-------------|----------------|-----|
| Rust toolchain | 1.85 (2024 edition) | Build from source |
| Python 3 | 3.11+ | amplihack SDK hooks run as Python subprocesses |
| `amplihack` Python package | any | Hooks import `amplihack` at runtime |
| Internet access | — | Default install clones from GitHub (skip with `--local`) |
| Node.js + npm (optional) | 18+ | Only needed when using the npm/npx wrapper path |

### macOS SIP Note {#macos-sip-note}

On macOS with System Integrity Protection (SIP) active, `deploy_binaries()` copies the running executable to `~/.local/bin`. SIP may quarantine the copied binary, making it non-executable. If `amplihack-hooks` fails to run after install, remove the quarantine attribute:

```sh
xattr -d com.apple.quarantine ~/.local/bin/amplihack-hooks
xattr -d com.apple.quarantine ~/.local/bin/amplihack
```

## Install Steps

### 1. Build the CLI binary

```sh
cd /path/to/amplihack-rs-update
cargo build --release
```

This produces two binaries:

- `target/release/amplihack` — the main CLI
- `target/release/amplihack-hooks` — the multicall hook dispatcher

### 2. Run the installer

```sh
./target/release/amplihack install
```

### Alternative: bootstrap through npx

If you want npm to provision the Rust CLI first, use the wrapper package:

```sh
npx --yes --package=git+https://github.com/rysweet/amplihack-rs.git -- amplihack install
```

The wrapper exposes the `amplihack` command, ensures both `amplihack` and
`amplihack-hooks` exist for the current platform, and then delegates to the same
native `amplihack install` flow. It prefers the matching GitHub release archive
and falls back to a local Cargo build when the packaged Rust workspace is present.

The installer performs these phases in order:

| Phase | What happens |
|-------|-------------|
| **Obtain framework** | Downloads and extracts the GitHub repository archive, or uses `--local` when requested. |
| **Deploy binaries** | Copies `amplihack` and `amplihack-hooks` to `~/.local/bin` with `0o755` permissions. |
| **Stage assets** | Copies framework files (agents, commands, tools, skills, etc.) to `~/.amplihack/.claude/`. |
| **Create runtime dirs** | Creates `~/.amplihack/.claude/runtime/` subdirectories with `0o755` permissions. |
| **Wire hooks** | Updates `~/.claude/settings.json` with the [7 amplihack hooks](../reference/hook-specifications.md). Backs up the existing file first. |
| **Verify** | Confirms the required staged framework assets are present. |
| **Write manifest** | Saves `~/.amplihack/.claude/install/amplihack-manifest.json` for use by `uninstall`. |

### 3. Verify the install

```sh
# Check binaries are on PATH
amplihack --version
amplihack-hooks --version

# If ~/.local/bin is not in $PATH, the installer printed an advisory:
#   ⚠️  ~/.local/bin is not in $PATH
#   Add: export PATH="$HOME/.local/bin:$PATH"
```

### 4. Check hook registration

Open `~/.claude/settings.json` and confirm the `hooks` section contains entries for `SessionStart`, `Stop`, `PreToolUse`, `PostToolUse`, `UserPromptSubmit`, and `PreCompact`. See the [Hook Specifications reference](../reference/hook-specifications.md) for the expected format.

## What Gets Installed Where

```
~/.local/bin/
├── amplihack            # main CLI binary
└── amplihack-hooks      # multicall hook dispatcher

~/.amplihack/.claude/
├── agents/amplihack/    # agent prompts
├── commands/amplihack/  # slash commands
├── tools/amplihack/
│   └── hooks/           # staged compatibility assets
├── context/             # shared context files
├── workflow/            # workflow definitions
├── skills/              # skill definitions
├── templates/           # project templates
├── scenarios/           # test scenarios
├── docs/                # bundled docs
├── schemas/             # JSON schemas
├── config/              # configuration
├── AMPLIHACK.md         # main instructions
└── install/
    └── amplihack-manifest.json  # uninstall manifest

~/.claude/
├── settings.json        # updated with hook registrations
└── settings.json.backup.<unix_seconds>  # backup of previous settings
```

## Troubleshooting

**`amplihack-hooks not found`**

The installer searches for `amplihack-hooks` in this order:
1. `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` environment variable
2. Sibling of the running `amplihack` executable
3. `PATH` lookup
4. `~/.local/bin/amplihack-hooks`
5. `~/.cargo/bin/amplihack-hooks`

Build `amplihack-hooks` before running the installer:

```sh
cargo build --release --bin amplihack-hooks
```

See [Binary Resolution](../reference/binary-resolution.md) for the full lookup sequence.

**`⚠️  ~/.local/bin is not in $PATH`**

This is a warning, not an error. Hook execution is unaffected because hooks are registered by absolute path. Add the directory to your shell profile when convenient:

```sh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

**Re-running install**

Install is idempotent. Re-running updates hook registrations in place without duplicating entries. Existing settings (permissions, directories) are preserved. See [Idempotent Installation](../concepts/idempotent-installation.md).

## See Also

- [Install from a Local Repository](./local-install.md) — offline install without git clone
- [Uninstall amplihack](./uninstall.md) — clean removal
- [amplihack install reference](../reference/install-command.md) — all flags and options
