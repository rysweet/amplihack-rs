---
title: "Amplifier Command Reference"
description: "Reference for launching Microsoft Amplifier through amplihack, including the argv-only prompt delivery contract."
last_updated: 2026-06-05
review_schedule: quarterly
owner: rysweet
doc_type: reference
---

# Amplifier Command Reference

Launch Microsoft Amplifier with the amplihack bundle for enhanced AI-assisted development.

## Synopsis

```bash
amplihack amplifier [OPTIONS] [-- AMPLIFIER_ARGS]
```

Pass Amplifier CLI arguments after `--`. Amplifier's documented
non-interactive prompt shape is `run [PROMPT]`:

```bash
amplihack amplifier -- run "Summarize this repository"
```

## Description

The `amplihack amplifier` command launches Microsoft Amplifier with the
amplihack bundle automatically loaded. Amplifier is a multi-model AI development
assistant that supports Claude, GPT-4, and other models through a unified
interface.

## Contents

- [Prerequisites](#prerequisites)
- [Options](#options)
- [Prompt delivery](#prompt-delivery)
- [Usage examples](#usage-examples)
- [Bundle discovery](#bundle-discovery)
- [Installation behavior](#installation-behavior)
- [Environment variables](#environment-variables)
- [Exit codes](#exit-codes)
- [Troubleshooting](#troubleshooting)
- [Comparison with other commands](#comparison-with-other-commands)
- [Related documentation](#related-documentation)

**Key features:**

- Automatically discovers and loads the amplihack bundle
- Auto-installs Amplifier via `uv` if not present
- Passes Amplifier provider, model, session, and output options through after `--`
- Session resume capability
- Integration with amplihack's auto mode

## Prerequisites

| Requirement   | Details                                                                                      |
| ------------- | -------------------------------------------------------------------------------------------- |
| **uv**        | Required for auto-installation. Install from [docs.astral.sh/uv](https://docs.astral.sh/uv/) |
| **Amplifier** | Auto-installed on first run via `uv tool install`                                            |
| **API Keys**  | Configure for your chosen provider (Anthropic, OpenAI, Azure)                                |

## Options

### amplihack wrapper options

| Option | Description |
| --- | --- |
| `--no-reflection` | Disable post-session reflection analysis. |
| `--subprocess-safe` | Skip shared launcher staging and environment updates for subprocess delegates. |
| `--docker` | Run through the amplihack Docker launcher. |
| `--append TEXT` | Append instructions to a running auto-mode session and exit. |
| `--auto` | Run in autonomous agentic mode with iterative loop execution. |
| `--max-turns N` | Maximum auto-mode turns. Default: `10`. |
| `--ui` | Enable interactive UI mode for auto mode. |

### Amplifier passthrough options

Use `--` before Amplifier arguments so they are passed to the `amplifier`
binary unchanged.

```bash
amplihack amplifier -- run --provider openai --model gpt-4o "Explain src/main.rs"
```

Common Amplifier `run` options:

| Option | Description |
| --- | --- |
| `--provider PROVIDER` | LLM provider to use. |
| `--model MODEL` | Model to use. |
| `--mode chat\|single` | Execution mode. |
| `--resume SESSION_ID` | Resume a specific session with a new prompt. |
| `--output-format text\|json\|json-trace` | Response output format. |
| `--max-tokens N` | Maximum output tokens. |

## Prompt delivery

Amplifier task prompts are delivered through structured argv. The stable
upstream contract is the positional `PROMPT` argument on `amplifier run`:

```text
amplifier run [OPTIONS] [PROMPT]
```

`amplihack` does not use shell interpolation for Amplifier prompts. Prompt text
is passed as a `std::process::Command` argument, so apostrophes, quotes,
semicolons, dollar signs, and newlines are not interpreted by a shell.

Amplifier does not currently have a documented task-prompt file or stdin
contract:

| Delivery mode | Amplifier support | Behavior |
| --- | --- | --- |
| `argv` | Supported | The prompt is passed as one structured argv value. |
| `tempfile` | Unsupported | Explicit requests fail before spawning `amplifier`. No prompt tempfile is created. |
| `stdin` | Unsupported | Explicit requests fail before spawning `amplifier`. No prompt bytes are written to stdin. |

### Prompt delivery configuration

`AMPLIHACK_PROMPT_DELIVERY` controls the requested delivery mode for migrated
subprocess launch paths. For Amplifier, the capability policy is intentionally
narrow:

| Setting | Amplifier result |
| --- | --- |
| unset, empty, or `auto` | Use structured argv delivery. |
| `argv` | Use structured argv delivery. |
| `tempfile` | Fail before launching Amplifier. |
| `stdin` | Fail before launching Amplifier. |

Examples:

```bash
# Default: Amplifier receives the prompt as one positional argv value.
amplihack amplifier -- run "Summarize this repository"

# Explicit compatibility mode: same argv delivery, useful in diagnostics.
AMPLIHACK_PROMPT_DELIVERY=argv \
  amplihack amplifier -- run "Summarize this repository"

# Invalid for Amplifier: fails before spawning the child process.
AMPLIHACK_PROMPT_DELIVERY=tempfile \
  amplihack amplifier -- run "Summarize this repository"
```

`AMPLIHACK_PROMPT_DELIVERY` is a request, not a capability override:

```bash
AMPLIHACK_PROMPT_DELIVERY=tempfile \
  amplihack amplifier -- run "Review this repository"
```

This command fails before launching Amplifier. The same hard rejection applies
to `AMPLIHACK_PROMPT_DELIVERY=stdin`. Amplifier remains `argv`-only until
upstream documents a stable `run` prompt-file or stdin task-prompt contract.
Use `amplihack doctor` to inspect the capability report.

The Amplifier launch path must build the upstream command as
`amplifier run [OPTIONS] [PROMPT]`. It must not add a synthetic `--prompt` flag
unless upstream Amplifier documents that flag as a stable task-prompt contract.

## Usage Examples

### Interactive Session

Start an interactive Amplifier session with the amplihack bundle:

```bash
amplihack amplifier
```

**Output:**

```
Using amplihack bundle: /path/to/amplifier-bundle
[Amplifier starts in interactive mode]
```

### Non-Interactive with Prompt

Execute a single task:

```bash
amplihack amplifier -- run "Explain the authentication flow in this codebase"
```

The prompt remains a single argv value even when it contains shell metacharacters
after parent-shell expansion:

```bash
amplihack amplifier -- run "Review this safely: don't expand \$HOME; keep 'quotes'"
```

### Single Response Mode

Get a single response without tool use:

```bash
amplihack amplifier -- run --mode single "What does this function do?"
```

### Model Selection

Use a specific model:

```bash
# Use GPT-4o
amplihack amplifier -- run --provider openai --model gpt-4o "Summarize this repository"

# Use Claude Sonnet
amplihack amplifier -- run --provider anthropic --model claude-sonnet-4-20250514 "Summarize this repository"
```

### Resume a Session

Continue a previous session:

```bash
amplihack amplifier -- run --resume session_20250114_120000_abc123 "Continue the previous task"
```

### Auto Mode

Run autonomously with a task:

```bash
amplihack amplifier --auto "Implement user authentication with JWT tokens"

# With extended turns for complex tasks
amplihack amplifier --auto --max-turns 25 "Refactor the entire API layer"
```

### Long Prompts

Amplifier has no supported long-form file or stdin task-prompt channel. Long
prompts therefore remain argv delivery when the requested mode is unset, `auto`,
or `argv`.

Do not document or rely on local shell substitutions such as
`"$(cat docs/feature-request.md)"` as an Amplifier long-prompt workaround. That
form expands in the parent shell before `amplihack` starts and still delivers
the resulting prompt to Amplifier through argv.

For Amplifier, `AMPLIHACK_PROMPT_DELIVERY=tempfile` and
`AMPLIHACK_PROMPT_DELIVERY=stdin` must fail before launch rather than silently
moving the prompt back to argv.

## Bundle Discovery

The command automatically finds the amplihack bundle using this search order:

1. **Current directory**: `./amplifier-bundle/bundle.md`
2. **Package location**: Searches up to 5 parent directories from the installed package

If no bundle is found, Amplifier runs without it and displays a warning:

```
Warning: amplihack bundle not found. Running Amplifier without bundle.
  Expected location: ./amplifier-bundle/bundle.md
```

## Installation Behavior

On first run, if Amplifier is not installed:

```
Installing Amplifier from git+https://github.com/microsoft/amplifier...
This will install Amplifier as a uv tool.
Continue? [y/N] y
✓ Amplifier CLI installed
```

**Non-interactive mode** (CI/scripts): Installation proceeds automatically without prompting.

## Environment Variables

| Variable | Description |
| --- | --- |
| `AMPLIHACK_PROMPT_DELIVERY` | Requested prompt delivery mode: `auto`, `argv`, `tempfile`, or `stdin`. Amplifier supports `argv` only; explicit `tempfile` and `stdin` requests fail before launch. |
| `AMPLIHACK_DEBUG` | Set to `true` for debug output. Do not rely on debug output for prompt privacy. |
| `ANTHROPIC_API_KEY` | API key for Anthropic models. |
| `OPENAI_API_KEY` | API key for OpenAI models. |
| `AZURE_OPENAI_*` | Azure OpenAI configuration. |

## Exit Codes

| Code | Meaning                                              |
| ---- | ---------------------------------------------------- |
| `0`  | Success                                              |
| `1`  | Error (installation failed, command not found, unsupported prompt delivery request, etc.) |

## Troubleshooting

### Amplifier not found after installation

**Problem**: Installation succeeds but `amplifier` command not found.

**Solution**: Ensure `~/.local/bin` (or uv's tool bin directory) is in your PATH:

```bash
# Add to ~/.bashrc or ~/.zshrc
export PATH="$HOME/.local/bin:$PATH"
```

### Bundle not found

**Problem**: Warning about missing bundle appears.

**Solution**:

1. Run from the amplihack repository root
2. Or ensure `amplifier-bundle/bundle.md` exists in your project

### uv not found

**Problem**: Error message about missing `uv`.

**Solution**: Install uv:

```bash
# macOS/Linux
curl -LsSf https://astral.sh/uv/install.sh | sh

# Windows
powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"
```

### API key errors

**Problem**: Authentication errors when running.

**Solution**: Set the appropriate API key for your provider:

```bash
# For Anthropic (Claude models)
export ANTHROPIC_API_KEY="your-key"

# For OpenAI (GPT models)
export OPENAI_API_KEY="your-key"
```

### Requested tempfile or stdin delivery fails

**Problem**: `AMPLIHACK_PROMPT_DELIVERY=tempfile` or
`AMPLIHACK_PROMPT_DELIVERY=stdin` fails before Amplifier starts.

**Solution**: This is expected. Amplifier documents positional prompt delivery
with `amplifier run [PROMPT]` and does not document a prompt-file or stdin
task-prompt contract. Use unset, `auto`, or `argv` delivery for Amplifier, or run
doctor to inspect the capability policy:

```bash
AMPLIHACK_PROMPT_DELIVERY=tempfile amplihack doctor
```

Use a different agent binary with a verified long-form prompt contract if the
prompt must stay out of child argv.

## Comparison with Other Commands

| Command               | Use Case                                            |
| --------------------- | --------------------------------------------------- |
| `amplihack amplifier` | Multi-model support, Amplifier features             |
| `amplihack claude`    | Claude Code with amplihack hooks and power steering |
| `amplihack copilot`   | GitHub Copilot CLI integration                      |
| `amplihack codex`     | OpenAI Codex CLI integration                        |

Choose `amplifier` when you need:

- Multi-model flexibility (switch between Claude, GPT-4, etc.)
- Amplifier-specific features
- The amplihack bundle context without full Claude Code hooks

## Related Documentation

- [Command Selection Guide](../commands/COMMAND_SELECTION_GUIDE.md) - Choose the right command
- [amplihack-rs Parity Reference](../amplihack-rs-parity.md) - Prompt delivery capability matrix, configuration, doctor diagnostics, and Rust API
- [Auto Mode Guide](../concepts/auto-mode.md) - Autonomous execution details
- [Launcher Model Configuration](./LAUNCHER_MODEL_CONFIGURATION.md) - Model selection for Claude launcher
