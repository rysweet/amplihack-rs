# amplihack uvx-help — Command Reference

## Synopsis

```
amplihack uvx-help [OPTIONS]
```

## Description

Displays UVX deployment information. UVX is the packaging mechanism used when
amplihack is installed via `uvx` (the Python package runner). This command helps
diagnose UVX-specific deployment paths and staging configuration.

The command name is kebab-case: `uvx-help` (set via explicit
`#[command(name = "uvx-help")]` attribute).

## Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--find-path` | bool | `false` | Print the detected UVX installation path and exit. |
| `--info` | bool | `false` | Show UVX staging information including deployment details. |

When neither flag is provided, the command prints general UVX help text.

## Examples

```sh
# Show general UVX help
amplihack uvx-help

# Find the UVX installation path
amplihack uvx-help --find-path

# Show staging information
amplihack uvx-help --info
```

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | Error (e.g. UVX deployment not detected) |

## Related

- [Binary Resolution](./binary-resolution.md) — How amplihack finds tool binaries
- [Install Command](./install-command.md) — Installation methods including UVX
