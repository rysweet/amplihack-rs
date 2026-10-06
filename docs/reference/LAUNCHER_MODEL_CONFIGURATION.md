# Launcher Model Configuration

## Default Model Behavior

When you do not name a model, the amplihack launcher passes
`--model claude-opus-5[1m]` to Claude Code. This is a concrete model id, not an
alias. Because amplihack passes a `--model`, its choice outranks the `"model"`
in your `~/.claude/settings.json`; set `AMPLIHACK_DEFAULT_MODEL` to an empty
value if you want that file to decide.

The default is a concrete id on purpose (issue #1421). amplihack used to
hardcode the alias `opus[1m]`. An alias is resolved by Claude Code, not by
amplihack, and what it resolves to changes with Claude Code's version: on one
install it resolved to a retired model id, so every agent step failed with a
404 naming a model the user had never chosen and could not find in any config
file. A concrete id cannot drift like that. A given Claude Code version either
accepts it or fails naming that exact string.

## Model Selection Priority

When the launcher determines which model to pass, it follows this strict priority
order:

1. **--model Flag** (highest priority)
   - Explicitly specified model via command-line flag
   - Example: `amplihack launch --model claude-sonnet-4-5`
   - Overrides the environment variable
   - Forwarded exactly as typed. A dotted Claude id such as `claude-opus-5.5`
     is not rewritten, but amplihack prints a warning naming the hyphenated
     spelling; see
     [`AMPLIHACK_DEFAULT_MODEL`](./environment-variables.md#amplihack_default_model)
     for when

2. **LiteLLM gateway**
   - When `AMPLIHACK_LITELLM_ENDPOINT`, `AMPLIHACK_LITELLM_API_KEY` or
     `AMPLIHACK_LITELLM_MODEL` is set (the
     [gateway variables](./environment-variables.md#external-litellm-gateway-variables)),
     the model is `AMPLIHACK_LITELLM_MODEL`, passed unchanged
   - `AMPLIHACK_DEFAULT_MODEL` is not read on this path
   - Other `AMPLIHACK_LITELLM_*` variables, such as
     `AMPLIHACK_LITELLM_TELEMETRY_FILE` (used by `amplihack litellm`
     verification), do not select this path

3. **AMPLIHACK_DEFAULT_MODEL Environment Variable**
   - Set in your shell environment
   - Example: `export AMPLIHACK_DEFAULT_MODEL=claude-sonnet-4-5`
   - An empty or whitespace-only value passes no `--model`, so Claude Code and
     `~/.claude/settings.json` decide
   - A dotted Claude id (`claude-opus-5.5`, the spelling GitHub Copilot CLI
     uses) is rewritten to the hyphenated id Claude Code accepts
     (`claude-opus-5-5`), issue #1527

4. **Built-in default** (`claude-opus-5[1m]`)
   - Used when `AMPLIHACK_DEFAULT_MODEL` is unset

Whenever amplihack passes a `--model` it did not get from your command line, it
prints one line to stderr naming the model and where it came from.

## Usage Examples

### Using the Built-in Default

```bash
# Passes --model claude-opus-5[1m]
amplihack launch
```

### Letting Claude Code Decide

```bash
# Passes no --model; Claude Code (and ~/.claude/settings.json) decide
AMPLIHACK_DEFAULT_MODEL= amplihack launch
```

### Override with Command-Line Flag

```bash
# Use a specific model with the 1M-token context window
amplihack launch --model 'claude-opus-5-5[1m]'

# Use an alias that Claude Code resolves
amplihack launch --model haiku
```

### Override with Environment Variable

```bash
# Set default model for all amplihack sessions
export AMPLIHACK_DEFAULT_MODEL='claude-opus-5-5[1m]'

# Now all launches use it
amplihack launch

# Still can override with flag
amplihack launch --model haiku
```

## Available Models

Claude Code owns the model catalogue and resolves aliases such as `opus[1m]`,
`sonnet`, and `haiku`. Which concrete model each alias maps to changes with the
Claude Code version you have installed, so this page deliberately does not
tabulate them; a table here would go stale exactly the way the old hardcoded
alias did. Run `claude --help`, or consult the Claude Code release notes, for
the models your install supports.

Claude Code's full model ids use hyphens (`claude-opus-5-5`), not dots.
amplihack rewrites a dotted id in `AMPLIHACK_DEFAULT_MODEL`, but not one given to
`--model`, where it warns instead.

The `[1m]` suffix requests the 1M-token context window where the model offers
one. If your workflow depends on it, pin it explicitly:

```bash
export AMPLIHACK_DEFAULT_MODEL='claude-opus-5-5[1m]'
```

## Configuration Persistence

Model selection is **per-session only**. Each time you launch amplihack, the priority hierarchy is evaluated fresh:

- Command-line flags apply to that session only
- Environment variables persist across shell sessions (until unset)
- With neither set, amplihack passes its built-in default, `claude-opus-5[1m]`

**To permanently change your default model**, set the environment variable in your shell profile:

```bash
# Add to ~/.bashrc or ~/.zshrc
export AMPLIHACK_DEFAULT_MODEL='claude-opus-5-5[1m]'

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
2. Verify it is not empty or whitespace-only, which passes no `--model` at all
3. Check for command-line flags that override it
4. Check whether `AMPLIHACK_LITELLM_ENDPOINT`, `AMPLIHACK_LITELLM_API_KEY` or
   `AMPLIHACK_LITELLM_MODEL` is set: while any of those three is set,
   `AMPLIHACK_DEFAULT_MODEL` is not read. Other `AMPLIHACK_LITELLM_*`
   variables make no difference here
5. Ensure you've reloaded your shell after setting it
6. Look for amplihack's own stderr line naming the model it passed:
   `amplihack: passing \`--model ...\` to \`claude\` (from AMPLIHACK_DEFAULT_MODEL)`

### A dotted model id such as `claude-opus-5.5` does not work

**Problem**: You passed a dotted Claude id such as `--model claude-opus-5.5`,
and the launch failed or the session is not running the model you named.
Issue #1527 records what one Claude Code version did with it.

**Solution**: Use the hyphenated id, `claude-opus-5-5`. amplihack forwards an
explicit `--model` unchanged and prints a warning naming the hyphenated
spelling; look for a stderr line beginning `amplihack: warning: passing`.

### Model not found (404)

**Problem**: Every step fails with
`API Error: 404 {"type":"not_found_error","message":"model: <some id>"}`.

**Solution**: The model you pinned, or amplihack's built-in default, is one your
account or your Claude Code version cannot reach. Pin a model your install
supports, or set `AMPLIHACK_DEFAULT_MODEL=` (empty) to let Claude Code choose.
If the id in the error is one you never chose, read amplihack's stderr line
naming the model it passed and where it came from, then check any `--model` in
your command line and `~/.claude/settings.json`.

## Related Documentation

- [`AMPLIHACK_DEFAULT_MODEL`](./environment-variables.md#amplihack_default_model) - Full reference for the variable, the dotted-id rewrite and the stderr lines
- [Launch Flag Injection](./launch-flag-injection.md) - How amplihack assembles the launch command line
- [Statusline Reference](./STATUSLINE.md) - Session information display
- [Auto Mode](../concepts/auto-mode.md) - Autonomous mode with model selection
