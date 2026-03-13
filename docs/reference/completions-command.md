# amplihack completions — Command Reference

## Synopsis

```
amplihack completions <SHELL>
```

## Description

Writes a shell completion script for amplihack to stdout. Pipe the output into your shell's completion directory or source it from your shell init file to enable tab-completion for all amplihack subcommands, flags, and arguments.

Completions are generated at runtime by `clap_complete` using amplihack's live command definition, so the output always reflects the installed binary's actual flags — including any subcommands added in future releases.

## Arguments

| Argument | Required | Values | Description |
|----------|----------|--------|-------------|
| `<SHELL>` | yes | `bash` `zsh` `fish` `powershell` | Target shell for completion script generation |

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Completion script written to stdout |
| `1` | Unrecognised shell name (see `--help`) |

## Output

The command writes the completion script to stdout and nothing else. No trailing newline is added beyond what `clap_complete` generates. Redirect stdout to install permanently; source it to activate for the current session only.

## Examples

### Bash

Persist completions for all future sessions:

```sh
amplihack completions bash > ~/.local/share/bash-completion/completions/amplihack
```

Or source for the current session only:

```sh
source <(amplihack completions bash)
```

### Zsh

Add to a directory on your `$fpath`:

```sh
amplihack completions zsh > ~/.zfunc/_amplihack
```

Then ensure `~/.zfunc` is on `$fpath` in `~/.zshrc` (before `compinit`):

```sh
fpath=(~/.zfunc $fpath)
autoload -U compinit && compinit
```

### Fish

```sh
amplihack completions fish > ~/.config/fish/completions/amplihack.fish
```

Fish auto-sources all files in `~/.config/fish/completions/`, so no further steps are needed.

### PowerShell

```powershell
amplihack completions powershell | Out-String | Invoke-Expression
```

To persist across sessions, append to your profile:

```powershell
amplihack completions powershell >> $PROFILE
```

## Verification

After installing completions, confirm they are active by typing `amplihack ` and pressing Tab:

```
$ amplihack <TAB>
claude        completions   copilot       doctor        install
launch        list          mode          run           show
uninstall     update        validate
```

Pressing Tab after a flag also completes values where they are enumerated:

```
$ amplihack completions <TAB>
bash  fish  powershell  zsh
```

## Implementation Note

Completions are generated from clap's `CommandFactory` trait using
`clap_complete::generate()`. The binary name hardcoded into every completion
script is `amplihack`. If you rename the binary, regenerate completions from
the renamed copy — the generated scripts embed the literal string `amplihack`
in completion function names and patterns.

## Related

- [How to Enable Shell Completions](../howto/enable-shell-completions.md) — Step-by-step guide for each shell
- [amplihack install](./install-command.md) — Full CLI reference for install and uninstall
