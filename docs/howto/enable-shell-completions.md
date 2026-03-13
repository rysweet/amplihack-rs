# How to Enable Shell Completions for amplihack

This guide installs tab-completion for amplihack subcommands, flags, and enumerated argument values in your shell. After following the steps for your shell, pressing Tab after `amplihack ` will complete subcommand names; pressing Tab after `amplihack completions ` will complete shell names.

## Prerequisites

You need a working `amplihack` binary on your `PATH`. Confirm with:

```sh
amplihack --version
# amplihack 0.9.1
```

## Bash

### Permanent (recommended)

```sh
amplihack completions bash > ~/.local/share/bash-completion/completions/amplihack
```

This directory is sourced automatically by `bash-completion` (version 2+). No changes to `~/.bashrc` are needed if you already have `bash-completion` installed.

If `~/.local/share/bash-completion/completions/` does not exist, create it first:

```sh
mkdir -p ~/.local/share/bash-completion/completions
amplihack completions bash > ~/.local/share/bash-completion/completions/amplihack
```

### System-wide (requires sudo)

```sh
amplihack completions bash | sudo tee /etc/bash_completion.d/amplihack > /dev/null
```

### Current session only

```sh
source <(amplihack completions bash)
```

### Verify

Open a new terminal (or run `source ~/.bashrc`) and type:

```
$ amplihack <TAB><TAB>
claude        completions   copilot       doctor        install
launch        list          mode          run           show
uninstall     update        validate
```

## Zsh

### Permanent (recommended)

Create a completions directory and add it to `$fpath` before `compinit`:

```sh
# 1. Create the directory
mkdir -p ~/.zfunc

# 2. Generate completions
amplihack completions zsh > ~/.zfunc/_amplihack

# 3. Add to ~/.zshrc (before the compinit line)
echo 'fpath=(~/.zfunc $fpath)' >> ~/.zshrc
echo 'autoload -U compinit && compinit' >> ~/.zshrc
```

If `~/.zshrc` already calls `compinit`, add only the `fpath` line — before the existing `compinit` call.

### Current session only

```sh
source <(amplihack completions zsh)
```

### Verify

Open a new terminal and type `amplihack ` followed by Tab. If completions are not active, run `compinit` manually and try again.

## Fish

```sh
amplihack completions fish > ~/.config/fish/completions/amplihack.fish
```

Fish sources all files in `~/.config/fish/completions/` automatically. Open a new terminal or run `exec fish` to activate.

### Verify

```
$ amplihack <TAB>
claude     (Launch Claude Code (alias))
completions  (Generate shell completion scripts)
copilot    (Launch GitHub Copilot CLI)
doctor     (Run system health checks)
install    (Install amplihack framework assets…)
…
```

Fish completions include the command descriptions from the CLI definition.

## PowerShell

### Current session only

```powershell
amplihack completions powershell | Out-String | Invoke-Expression
```

### Permanent

Append to your PowerShell profile so completions load automatically in every session:

```powershell
# Find your profile path
$PROFILE

# Append completions
amplihack completions powershell >> $PROFILE
```

If `$PROFILE` does not exist yet:

```powershell
New-Item -ItemType File -Path $PROFILE -Force
amplihack completions powershell >> $PROFILE
```

### Verify

Open a new PowerShell session and type `amplihack ` then press Tab.

## Regenerating Completions After Upgrade

Completions are generated at runtime from the installed binary's command definition. After upgrading amplihack, regenerate by re-running the same install command for your shell. The existing file will be overwritten.

Example (Bash):

```sh
amplihack completions bash > ~/.local/share/bash-completion/completions/amplihack
```

## Troubleshooting

**Tab produces nothing in Bash**

Confirm `bash-completion` is installed:

```sh
type _init_completion && echo "bash-completion active"
```

If not installed: `sudo apt install bash-completion` (Debian/Ubuntu) or `brew install bash-completion@2` (macOS).

**Zsh completions not found after regenerating**

Delete the `~/.zcompdump` cache and restart:

```sh
rm -f ~/.zcompdump
exec zsh
```

**Fish shows old completions**

Fish caches completions at startup. Run `exec fish` to reload, or:

```sh
rm ~/.config/fish/completions/amplihack.fish
amplihack completions fish > ~/.config/fish/completions/amplihack.fish
exec fish
```

## Related

- [amplihack completions — Command Reference](../reference/completions-command.md) — Full reference including all supported shells and output format
