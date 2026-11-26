---
name: github-copilot-cli
version: 1.0.0
description: Expert knowledge of GitHub Copilot CLI - installation, updating and version management, configuration, usage, MCP servers, skills, custom agents, and troubleshooting. Use when asking about copilot cli, installing or updating copilot, copilot version, gh copilot, copilot commands, MCP setup, or copilot extensibility.
source_urls:
  - https://docs.github.com/en/copilot/concepts/agents/about-copilot-cli
  - https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli
  - https://docs.github.com/en/copilot/how-tos/use-copilot-agents/use-copilot-cli
  - https://docs.github.com/en/copilot/concepts/agents/about-agent-skills
  - https://github.com/github/copilot-cli
---

# GitHub Copilot CLI Expert

**Comprehensive knowledge of GitHub Copilot CLI installation, configuration, usage, and extensibility.**

## Purpose

This skill provides expert guidance on:

1. Installing and updating GitHub Copilot CLI
2. Authentication and configuration
3. Core usage patterns and slash commands
4. Extensibility (MCP servers, skills, custom agents)
5. Troubleshooting common issues

## When I Activate

Automatically when you mention:

- "copilot cli", "github copilot cli", "gh copilot"
- "install copilot", "update copilot"
- "copilot commands", "copilot slash commands"
- "copilot mcp", "add mcp server"
- "copilot skills", "create copilot skill"
- "copilot custom agent"

## Quick Reference

### Installation (All Platforms)

**Always fetch latest version** - use these commands:

| Platform               | Command                                            |
| ---------------------- | -------------------------------------------------- |
| Windows (WinGet)       | `winget install GitHub.Copilot`                    |
| macOS/Linux (Homebrew) | `brew install copilot-cli`                         |
| All (npm, Node.js 22+) | `npm install -g @github/copilot`                   |
| macOS/Linux (Script)   | `curl -fsSL https://gh.io/copilot-install \| bash` |

**Prerelease versions**: Add `@prerelease` or use `GitHub.Copilot.Prerelease`

### Updating and Version Management

| Method   | Update command                 |
| -------- | ------------------------------ |
| Homebrew | `brew upgrade copilot-cli`     |
| npm      | `npm update -g @github/copilot` |
| WinGet   | `winget upgrade GitHub.Copilot` |
| Script   | Run the install script again   |

Check the version with `copilot --version`. Pinning a version and the release list are in reference.md.

### Authentication

```bash
copilot                    # Launch and follow /login prompt
# OR use PAT with "Copilot Requests" permission:
export GH_TOKEN=your_token
```

### Essential Slash Commands

| Command              | Purpose                                          |
| -------------------- | ------------------------------------------------ |
| `/help`              | Show all commands and shortcuts                  |
| `/model`             | Select from models available in the current runtime |
| `/agent`             | Browse and select available agents               |
| `/skills`            | Manage skills                                    |
| `/mcp`               | Manage MCP server configuration                  |
| `/delegate [prompt]` | Hand off to Copilot coding agent (creates PR)    |
| `/compact`           | Reduce context window usage                      |
| `/context`           | Show token usage visualization                   |

### Keyboard Shortcuts

| Shortcut | Action                              |
| -------- | ----------------------------------- |
| `@`      | Mention files to include in context |
| `!`      | Execute shell command directly      |
| `Esc`    | Cancel current operation            |
| `Ctrl+C` | Cancel / clear / exit               |
| `Ctrl+L` | Clear screen                        |
| `Ctrl+O` | Expand/collapse timeline            |

### Extensibility Overview

1. **MCP Servers** - Extend Copilot with external tools
2. **Skills** - Task-specific instructions and scripts
3. **Custom Agents** - Specialized agent profiles
4. **Custom Instructions** - Repository-wide guidance

## Navigation Guide

### When to Read Supporting Files

**reference.md** - Read when you need:

- Complete slash command reference with all options
- Detailed MCP server configuration and JSON schema
- Full permissions system documentation
- Advanced configuration options
- Environment variables reference
- Programmatic mode, updating and version management
- Troubleshooting in detail

**examples.md** - Read when you need:

- Step-by-step installation walkthroughs
- Working MCP server configurations
- Skill creation examples
- Custom agent examples
- Real-world usage scenarios
- GitHub integration and scripted (programmatic) examples

## Core Concepts

### MCP Servers (Model Context Protocol)

Copilot CLI includes GitHub MCP server by default. Add custom MCP servers:

```bash
/mcp add             # Interactive configuration
/mcp show            # List configured servers
/mcp edit <name>     # Modify existing server
```

Config stored in: `~/.copilot/mcp-config.json`

### Skills

Skills enhance Copilot's task performance with instructions and scripts.

**Locations**:

- Project: `.github/skills/` or `.claude/skills/`
- Personal: `~/.copilot/skills/` or `~/.claude/skills/`

**Structure**:

```
.github/skills/my-skill/
└── SKILL.md    # Instructions with YAML frontmatter
```

### Custom Agents

Specialized agent profiles stored as Markdown files.

**Locations**:

- User: `~/.copilot/agents/`
- Repository: `.github/agents/`
- Organization: `.github-private/agents/`

### Custom Instructions

Repository-specific guidance for Copilot:

- `.github/copilot-instructions.md` - Repository-wide
- `.github/instructions/**/*.instructions.md` - Path-specific
- `AGENTS.md` - Agent behavior instructions

### Programmatic Mode

`copilot -p "<prompt>"` runs one prompt and exits, for scripts and CI. Approve tools with `--allow-tool 'shell(git)'`, `--allow-all-tools` or `--deny-tool 'shell(rm)'`. Formats and examples are in reference.md.

### Security Model

- **Trusted directories**: Confirmed on first launch
- **Tool approval**: Required for file modifications
- **Path permissions**: Current directory, subdirectories and temp by default
- **URL permissions**: All URLs require approval

Flags and limits are under "Permissions System" in reference.md.

## Common Patterns

- `@path` includes a file in the prompt: `Fix the bug in @src/app.js`
- `/delegate <task>` hands the work to the Copilot coding agent, which opens a PR
- `copilot --continue` resumes the latest session; `copilot --resume` picks one
- `--allow-tool` and `--deny-tool` approve or refuse tools without prompts

## Troubleshooting Quick Tips

| Issue                  | Solution                                                 |
| ---------------------- | -------------------------------------------------------- |
| Not installed          | `brew install copilot-cli` or `npm i -g @github/copilot` |
| Not authenticated      | Run `/login` in Copilot CLI                              |
| Policy disabled        | Check org/enterprise Copilot settings                    |
| MCP server not loading | Verify `~/.copilot/mcp-config.json` syntax               |
| Skill not activating   | Restart Copilot CLI, check SKILL.md YAML                 |
| Not found after install | Restart terminal, check `PATH`, run `which copilot`     |
| "No subscription"      | Check plan at github.com/settings/copilot, then `/login` |
| Tool permission denied | `/add-dir <path>` or `--allow-all-paths`                 |
| Context window full    | `/compact`, or `/clear` to start again                   |

More detail is under "Troubleshooting" in reference.md.

---

**Version**: 1.0.0 | **Status**: Public Preview
**Note**: GitHub Copilot CLI is in public preview and subject to change.
