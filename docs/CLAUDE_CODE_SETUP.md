# Claude Code Setup Guide

How to get Claude Code running with amplihack.

## What is Claude Code?

Claude Code is Anthropic's official CLI for AI-assisted software development. It runs in your terminal, reads your codebase, and helps you write, debug, and refactor code. Amplihack extends Claude Code with structured workflows, specialized agents, and persistent memory.

## Getting an API Key

Claude Code requires an Anthropic API key OR a Claude Max/Pro subscription:

**Option A: Anthropic API (pay-per-use)**

1. Go to [console.anthropic.com](https://console.anthropic.com)
2. Sign up or log in
3. Navigate to API Keys and create a new key
4. Set in your shell: `export ANTHROPIC_API_KEY=sk-ant-...`

**Option B: Claude Max subscription (flat rate)**

If you have a Claude Max subscription ($100/month as of Feb 2026), Claude Code is included. Run `claude` and follow the OAuth login flow.

## Cost Expectations

Claude Code usage with amplihack varies by task complexity:

| Task Type             | Typical Cost (API) | Tokens Used |
| --------------------- | ------------------ | ----------- |
| Q&A / quick question  | $0.01-0.05         | 1K-5K       |
| Single bug fix        | $0.50-3.00         | 50K-300K    |
| Feature with workflow | $2.00-15.00        | 200K-1.5M   |
| Parallel workstreams  | $5.00-30.00        | 500K-3M     |

Claude Max subscription eliminates per-token costs for qualifying usage.

## Installation

```bash
# macOS
brew install --cask claude-code

# Linux / WSL
curl -fsSL https://claude.ai/install.sh | bash

# Windows
winget install Anthropic.ClaudeCode
```

Verify: `claude --version`

## Permanent Setup

Add to your shell profile (`~/.bashrc`, `~/.zshrc`, or `~/.profile`):

```bash
# API key (skip if using Claude Max subscription)
export ANTHROPIC_API_KEY=sk-ant-your-key-here

# Optional: enable native trace logging for debugging
export AMPLIHACK_TRACE_LOGGING=true
```

Then install amplihack:

```bash
# Try without installing (temporary)
uvx amplihack install

# Or install permanently
uv tool install amplihack
```

### How skill discovery is staged

Claude Code discovers each amplihack skill directly under
`~/.claude/skills/<skill-name>/SKILL.md`. The canonical source is the installed
`~/.amplihack/.claude/skills/` tree. Skills nested in source categories are
flattened by skill name at the discovery root, and their support files and
nested directories are copied with them. Internal support links are
materialized only when their resolved targets remain inside the canonical
skills tree. Known broken support links are skipped.

Amplihack records ownership outside the discovery directory. It replaces or
prunes only destinations whose complete contents still match that ownership
record. A user-owned file, directory, symlink, changed managed skill, malformed
ownership record, duplicate skill name, or path escape causes installation to
fail closed without overwriting content.

Skill refresh and stale-skill pruning are transactional: new trees are prepared
before live destinations are replaced, previous trees are backed up, and a
failed publication is rolled back so the next launch can retry. If rollback
itself cannot complete, the error reports the retained recovery directory.
Uninstall removes only unchanged, manifest-owned Claude skills and the
amplihack wrapper; replaced or unrelated user content is preserved.

Releases before direct skill discovery installed an amplihack wrapper containing
both `skills/` and `.claude-plugin/plugin.json`. That wrapper is migrated only
when its exact manifest and every present plugin asset match the legacy
amplihack-managed layout. Extra or changed content is treated as user-owned and
is left untouched. The replacement wrapper is prepared before the legacy
wrapper is moved, and failed replacement restores it for a later retry.

Claude staging does not change Copilot staging. Copilot skills continue to use
their existing `.copilot` installation and refresh paths.

## Verify Everything Works

```bash
# Check Claude Code
claude --version

# Check amplihack
amplihack

# Launch Claude Code with amplihack agents
amplihack claude
```

## Next Steps

- [Prerequisites](PREREQUISITES.md) for full tool setup
- [Tutorial](tutorials/amplihack-tutorial.md) for hands-on learning
- [README](../README.md) for project overview
