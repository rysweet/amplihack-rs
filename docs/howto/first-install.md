# How to Install amplihack for the First Time

This guide walks through running `amplihack install` on a machine that has never had amplihack installed. The command is self-contained: it validates prerequisites, deploys binaries, stages framework assets, and wires all Claude Code hooks in a single invocation.

## Prerequisites

| Requirement | Minimum version | Why |
|-------------|----------------|-----|
| Supported host | Linux or macOS, x86_64 or aarch64 | A matching published release archive must be available; on Windows, use WSL |
| Internet access | — | A first installation obtains release executables and any missing framework or tool sources; `--local` supplies framework assets only |
| Rust toolchain (optional) | Workspace toolchain | Needed to build a CLI from source; building does not authorise publishing that executable |
| Node.js + npm (optional) | Node.js 24+ | Only needed when using the npm/npx wrapper path |

### macOS SIP Note {#macos-sip-note}

Installation stages validated release executables in `~/.amplihack/releases/`
and publishes links in `~/.local/bin`. If macOS reports that an executable
from a trusted downloaded release is quarantined, remove its quarantine attribute:

```sh
xattr -d com.apple.quarantine ~/.local/bin/amplihack-hooks
xattr -d com.apple.quarantine ~/.local/bin/amplihack
```

## Install Steps

### 1. Obtain a release CLI

Download and unpack the archive for your supported host from
[amplihack-rs releases](https://github.com/rysweet/amplihack-rs/releases).
Use the unpacked CLI to start installation:

```sh
cd /path/to/unpacked-release
./amplihack --version
```

You may also build a CLI from source and invoke its explicit `install`
command. Installation still requires an eligible installed release of the
selected version or that exact published platform archive. A checkout and
its sibling binaries cannot replace those release executables. An unpublished
checkout version cannot be installed merely by compiling it. See
[Run a checkout build safely](run-checkout-safely.md).

### 2. Run the installer

```sh
./amplihack install
```

For guided setup (choose default tool, hook scope, update-check frequency):

```sh
./amplihack install --interactive
```

See [Interactive Install](./interactive-install.md) for a walkthrough of the wizard prompts.

### Alternative: bootstrap through npx

If you want npm to provision the Rust CLI first, use the wrapper package:

```sh
npx --yes --package=git+https://github.com/rysweet/amplihack-rs.git -- amplihack install
```

The wrapper exposes the `amplihack` command, ensures both `amplihack` and
`amplihack-hooks` exist for the current platform, and then delegates to the same
native `amplihack install` flow. It prefers the matching GitHub release archive
and falls back to a local Cargo build when the packaged Rust workspace is present.

Published release archives currently cover Linux and macOS on `x64`/`arm64`.
The wrapper's source-build fallback can provide a CLI to invoke, but cannot
provide release publication authority. Installation still requires eligible
installed release executables or the matching published platform archive.
An unsupported platform cannot bypass that requirement by building from
source. On Windows, use WSL and its Linux release target.

The installer performs these phases in order:

| Phase | What happens |
|-------|-------------|
| **Resolve release executables** | Validates required executables from an eligible installed release of the selected version, or obtains that exact published platform archive. Refuses before installation writes if they are unavailable. |
| **Obtain framework** | Uses a local source with the binary's version, or fetches the matching release tag, or uses `--local` when requested. See [Framework source selection](../reference/framework-source-selection.md). |
| **Publish release** | Stages validated release executables and the framework bundle under `~/.amplihack/releases/`, then switches the user-level binary and bundle links. |
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
├── amplihack            # link to the release CLI
└── amplihack-hooks      # link to the release hook dispatcher

~/.amplihack/releases/<release>/
├── bin/                # validated release executables
└── amplifier-bundle/   # framework bundle

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

**Required release executables unavailable or invalid**

Use a supported published release for your platform and restore access to its
archive. Installation requires both `amplihack` and `amplihack-hooks` from
the selected release; a local build, a hooks override or a companion on `PATH`
cannot substitute for them. `--local` changes framework assets only. See
[Release binary staging](../reference/install-command.md#release-binary-staging-internal)
and [offline preparation](local-install.md#prepare-an-offline-host).

**`⚠️  ~/.local/bin is not in $PATH`**

This is a warning, not an error. Hook execution is unaffected because hooks are registered by absolute path. Add the directory to your shell profile when convenient:

```sh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

**`⚠️  PATH conflict`**

The installer published release binary links in `~/.local/bin`, but another
`amplihack` or `amplihack-hooks` appears earlier on `PATH`. Follow
[Repair install/update PATH conflicts](repair-install-update-path-conflicts.md)
to reorder `PATH` or remove stale system binaries.

**Re-running install**

Install is idempotent. Re-running updates hook registrations in place without duplicating entries. Existing settings (permissions, directories) are preserved. See [Idempotent Installation](../concepts/idempotent-installation.md).

## See Also

- [Interactive Install](./interactive-install.md) — guided wizard for tool, scope, and update preferences
- [Install from a Local Repository](./local-install.md) — local framework assets and offline preparation
- [Repair install/update PATH conflicts](./repair-install-update-path-conflicts.md) — fix stale `/usr/local/bin` binaries
- [Uninstall amplihack](./uninstall.md) — clean removal
- [amplihack install reference](../reference/install-command.md) — all flags and options
