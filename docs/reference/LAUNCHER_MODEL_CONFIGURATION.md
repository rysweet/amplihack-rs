# Launcher Model Configuration

## Default Model Behavior

**The amplihack launcher does not choose a model.** When you do not name one, it
puts no `--model` on the command line and Claude Code applies its own current
default — which also means the `"model"` in your `~/.claude/settings.json` takes
effect.

This is deliberate (issue #1421). amplihack used to hardcode a model alias here.
An alias is resolved by Claude Code, not by amplihack, and what it resolves to
changes with Claude Code's version: one such alias resolved to a retired model
id, so every agent step failed with a 404 naming a model the user had never
chosen and could not find in any config file. amplihack does not own the model
catalogue, so it no longer pretends to. (This document itself is the other half
of the evidence: it claimed the default was `sonnet[1m]` long after the code had
moved to `opus[1m]`.)

## Model Selection Priority

When the launcher determines which model to pass, it follows this strict priority
order:

1. **--model Flag** (highest priority)
   - Explicitly specified model via command-line flag
   - Example: `amplihack launch --model opus`
   - Overrides the environment variable

2. **AMPLIHACK_DEFAULT_MODEL Environment Variable**
   - Set in your shell environment
   - Example: `export AMPLIHACK_DEFAULT_MODEL=opus`
   - Opt-in: this is the only way to pin a model for every launch
   - An empty or whitespace-only value is treated as unset
   - When it is used, amplihack prints a line to stderr naming the model and
     this variable, so a later "model not found" error is traceable

3. **Nothing** (the default)
   - No `--model` argument is passed; Claude Code picks
   - Cannot go stale, because amplihack is not choosing

## Usage Examples

### Using the Tool's Own Default

```bash
# Passes no --model; Claude Code (and ~/.claude/settings.json) decide
amplihack launch
```

### Override with Command-Line Flag

```bash
# Use Opus model with extended context
amplihack launch --model opus[1m]

# Use Haiku for quick tasks
amplihack launch --model haiku

# Use standard Sonnet without extended context
amplihack launch --model sonnet
```

### Override with Environment Variable

```bash
# Set default model for all amplihack sessions
export AMPLIHACK_DEFAULT_MODEL=opus[1m]

# Now all launches use Opus by default
amplihack launch

# Still can override with flag
amplihack launch --model haiku
```

## Available Models

Claude Code owns the model catalogue and resolves aliases such as `opus[1m]`,
`sonnet`, and `haiku`. Which concrete model each alias maps to changes with the
Claude Code version you have installed, so this page deliberately does not
tabulate them — a table here would go stale exactly the way the old hardcoded
default did. Run `claude --help`, or consult the Claude Code release notes, for
the aliases your install supports.

The `[1m]` suffix requests the 1M-token context window where the model offers
one. If your workflow depends on it, pin it explicitly:

```bash
export AMPLIHACK_DEFAULT_MODEL='opus[1m]'
```

## Configuration Persistence

Model selection is **per-session only**. Each time you launch amplihack, the priority hierarchy is evaluated fresh:

- Command-line flags apply to that session only
- Environment variables persist across shell sessions (until unset)
- With neither set, nothing is passed and Claude Code decides

**To permanently change your default model**, set the environment variable in your shell profile:

```bash
# Add to ~/.bashrc or ~/.zshrc
export AMPLIHACK_DEFAULT_MODEL=opus[1m]

# Reload shell configuration
source ~/.bashrc  # or source ~/.zshrc
```

## Checking Active Model

The active model is displayed in the statusline at the bottom of Claude Code:

```
~/src/amplihack (main → origin) Sonnet[1m] 🎫 234K 💰$1.23 ⏱12m
```

For more information about the statusline, see [STATUSLINE.md](./STATUSLINE.md).

## Troubleshooting

### Environment variable not being respected

**Problem**: You set `AMPLIHACK_DEFAULT_MODEL` but a different model is used.

**Solution**:

1. Verify the variable is exported: `echo $AMPLIHACK_DEFAULT_MODEL`
2. Verify it is not empty or whitespace-only — that is treated as unset
3. Check for command-line flags that override it
4. Ensure you've reloaded your shell after setting it
5. Look for amplihack's own stderr line naming the model it passed:
   `amplihack: passing \`--model ...\` to \`claude\` (from AMPLIHACK_DEFAULT_MODEL)`

### Model not found (404)

**Problem**: Every step fails with
`API Error: 404 {"type":"not_found_error","message":"model: <some id>"}`.

**Solution**: The alias you pinned resolved to a model your account or your
Claude Code version cannot reach. Unset `AMPLIHACK_DEFAULT_MODEL` and let Claude
Code choose, or pin an alias your install supports. If the id in the error is one
you never chose, check `AMPLIHACK_DEFAULT_MODEL`, any `--model` in your command
line, and `~/.claude/settings.json` — amplihack itself contributes no model id.

## Related Documentation

- [Statusline Reference](./STATUSLINE.md) - Session information display
- [Auto Mode](../concepts/auto-mode.md) - Autonomous mode with model selection
