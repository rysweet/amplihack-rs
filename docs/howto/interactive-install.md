# How to Install amplihack with the Interactive Wizard

The `--interactive` flag adds a guided setup wizard to `amplihack install`. The wizard prompts for three configuration choices: default launch tool, hook registration scope, and update-check frequency. The install itself is unchanged — all framework assets, binaries, and hooks are deployed regardless of your answers. The wizard only configures preferences.

## When to Use This

Use `--interactive` when you want to:

- Choose which tool (`claude`, `copilot`, or `codex`) a bare `amplihack` invocation launches
- Place hook registrations in a repo-local `.claude/settings.json` instead of the global one
- Set your preferred update-check cadence (daily, weekly, manual, or disabled)

If you want the default configuration without prompts, run `amplihack install` without the flag. The non-interactive path is unchanged.

## Run the Interactive Installer

```sh
amplihack install --interactive
```

The wizard presents three prompts in sequence:

### 1. Default Launch Tool

```
? Select the default tool for bare `amplihack` invocations:
> Claude Code
  GitHub Copilot
  OpenAI Codex CLI
```

This sets which AI tool starts when you run `amplihack` without a subcommand. All three tools are still installed and available via explicit subcommands (`amplihack claude`, `amplihack copilot`, `amplihack codex`) regardless of your choice.

### 2. Hook Registration Scope

```
? Where should Claude Code hooks be registered?
> Global (~/.claude)
  Repo-local (.claude)
```

**Global** writes hooks to `~/.claude/settings.json` (the default behavior).

**Repo-local** writes hooks to `<current-repo>/.claude/settings.json`. This requires the current working directory to be inside a git repository. If no git repository is found, the wizard prints a warning and falls back to global scope:

```
warning: repo-local scope requested but no .git found in /current/path; falling back to global scope
```

### 3. Update-Check Preference

```
? How often should amplihack check for updates?
> Auto (weekly)
  Auto (daily)
  Manual only
  Disabled
```

This preference is stored in the install manifest and read by the update-check system at launch time. See [Manage Tool Update Notifications](./manage-tool-update-checks.md) for details on the update-check behavior.

## Combine with --local

The `--interactive` and `--local` flags compose. `--local` controls where framework assets are sourced from; `--interactive` controls configuration preferences. Use both when installing from a local checkout with guided setup:

```sh
amplihack install --interactive --local /path/to/amplihack-clone
```

## Non-TTY Environments

If `--interactive` is passed but stdin is not a terminal (piped input, CI pipelines, Docker builds), the wizard cannot display prompts. Instead of failing, it prints a warning to stderr and proceeds with default configuration:

```
warning: --interactive requires a terminal; falling back to default configuration
```

Exit code is `0`. This matches the graceful degradation pattern used elsewhere in the CLI.

## What the Wizard Does NOT Change

The wizard configures preferences only. It does not:

- Limit which binaries are installed (all three tools are always deployed)
- Skip any install phases (assets, hooks, manifest are always written)
- Change the install location (`~/.amplihack/.claude/` is always the staging root)
- Affect `amplihack uninstall` behavior

## Where Preferences Are Stored

Wizard choices are persisted in the install manifest at `~/.amplihack/.claude/install/amplihack-manifest.json` as two optional fields:

| Field | Values | Default (non-interactive) |
|-------|--------|---------------------------|
| `default_tool` | `"claude"`, `"copilot"`, `"codex"` | absent (equivalent to `"claude"`) |
| `update_check_preference` | `"auto-weekly"`, `"auto-daily"`, `"manual"`, `"disabled"` | absent (equivalent to `"auto-weekly"`) |

Hook scope is applied at install time by writing to the chosen `settings.json` location. It is not stored as a manifest field.

See [Install Manifest](../reference/install-manifest.md) for the full schema.

## See Also

- [Install amplihack for the First Time](./first-install.md) — standard (non-interactive) install walkthrough
- [amplihack install reference](../reference/install-command.md) — all flags, exit codes, and phases
- [Manage Tool Update Notifications](./manage-tool-update-checks.md) — update-check behavior details
- [Run amplihack in Non-interactive Mode](./run-in-noninteractive-mode.md) — CI and headless usage
