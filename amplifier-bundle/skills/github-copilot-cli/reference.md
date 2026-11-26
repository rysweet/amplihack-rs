# GitHub Copilot CLI Reference

Complete technical reference for GitHub Copilot CLI.

**Last Updated**: 2026-01-25

---

## Table of Contents

1. [Installation Reference](#installation-reference)
2. [Authentication](#authentication)
3. [Slash Commands Reference](#slash-commands-reference)
4. [Keyboard Shortcuts](#keyboard-shortcuts)
5. [MCP Server Configuration](#mcp-server-configuration)
6. [Skills System](#skills-system)
7. [Custom Agents](#custom-agents)
8. [Custom Instructions](#custom-instructions)
9. [Permissions System](#permissions-system)
10. [Environment Variables](#environment-variables)
11. [Configuration Files](#configuration-files)
12. [Command Line Options](#command-line-options)
13. [Programmatic Mode](#programmatic-mode)
14. [Updating and Version Management](#updating-and-version-management)
15. [Troubleshooting](#troubleshooting)

---

## Installation Reference

### Prerequisites

- **Active Copilot subscription**: Pro, Pro+, Business, or Enterprise
- **Windows**: PowerShell v6 or higher
- **npm install**: Node.js 22+
- **Organization/Enterprise**: Copilot CLI policy must be enabled

### Installation Methods

#### WinGet (Windows)

```powershell
# Stable
winget install GitHub.Copilot

# Prerelease
winget install GitHub.Copilot.Prerelease

# Update
winget upgrade GitHub.Copilot
```

#### Homebrew (macOS/Linux)

```bash
# Stable
brew install copilot-cli

# Prerelease
brew install copilot-cli@prerelease

# Update
brew upgrade copilot-cli
```

#### npm (All Platforms, Node.js 22+)

```bash
# Stable
npm install -g @github/copilot

# Prerelease
npm install -g @github/copilot@prerelease

# Update
npm update -g @github/copilot

# Check version
copilot --version
```

#### Install Script (macOS/Linux)

```bash
# Default (installs to ~/.local/bin)
curl -fsSL https://gh.io/copilot-install | bash

# With sudo (installs to /usr/local/bin)
curl -fsSL https://gh.io/copilot-install | sudo bash

# Specific version to custom directory
curl -fsSL https://gh.io/copilot-install | VERSION="v0.0.369" PREFIX="$HOME/custom" bash

# Using wget
wget -qO- https://gh.io/copilot-install | bash
```

**Environment Variables for Install Script**:

- `VERSION`: Specific version to install (default: latest)
- `PREFIX`: Installation directory (default: `~/.local` or `/usr/local` for root)

#### Direct Download

Download from [GitHub Releases](https://github.com/github/copilot-cli/releases/) and extract.

---

## Authentication

### Browser-based Login

```bash
copilot          # Launch CLI
/login           # Follow browser authentication flow
```

### Personal Access Token (PAT)

1. Create fine-grained PAT at https://github.com/settings/personal-access-tokens/new
2. Add **Copilot Requests** permission
3. Set environment variable:

```bash
export GH_TOKEN=ghp_xxxxxxxxxxxx
# OR
export GITHUB_TOKEN=ghp_xxxxxxxxxxxx
```

**Precedence**: `GH_TOKEN` > `GITHUB_TOKEN`

### Logout

```bash
/logout
```

---

## Slash Commands Reference

### Session Management

| Command               | Description                             |
| --------------------- | --------------------------------------- |
| `/clear`, `/new`      | Clear conversation history              |
| `/compact`            | Summarize history to reduce context     |
| `/context`            | Show token usage visualization          |
| `/exit`, `/quit`      | Exit the CLI                            |
| `/session [checkpoints [n]\|files\|plan\|rename <name>]` | Show session info, checkpoints, files or plan |
| `/resume [sessionId]` | Switch to different session             |
| `/rename <name>`      | Rename current session                  |
| `/usage`              | Display session usage metrics           |

### Navigation

| Command                   | Description                      |
| ------------------------- | -------------------------------- |
| `/cwd [directory]`, `/cd` | Change or show working directory |
| `/add-dir <directory>`    | Add directory to allowed list    |
| `/list-dirs`              | Display allowed directories      |

### Model & Agents

| Command              | Description                      |
| -------------------- | -------------------------------- |
| `/model [model]`, `/models` | Select AI model           |
| `/agent`             | Browse and select agents         |
| `/delegate [prompt]` | Hand off to Copilot coding agent |
| `/plan [prompt]`     | Create implementation plan       |
| `/review [prompt]`   | Run code review agent            |

**Available Models** (use `/model` to see current list):

Use the runtime’s current model list and configured default; availability depends on the account and installation.

Each prompt costs premium requests multiplied by the model's multiplier. `/model` shows the current multipliers.

### Tools & Extensions

| Command                                           | Description            |
| ------------------------------------------------- | ---------------------- |
| `/mcp [show\|add\|edit\|delete\|disable\|enable]` | Manage MCP servers     |
| `/skills [list\|info\|add\|remove\|reload]`       | Manage skills          |
| `/reset-allowed-tools`                            | Reset tool permissions |

### Utility

| Command                      | Description                            |
| ---------------------------- | -------------------------------------- |
| `/help`                      | Show all commands                      |
| `/feedback`                  | Provide feedback                       |
| `/diff`                      | Review changes in current directory    |
| `/share [file\|gist] [path]` | Share session to file or gist          |
| `/ide`                       | Connect to IDE workspace               |
| `/login`, `/logout`          | Authentication                         |
| `/user [show\|list\|switch]` | Manage GitHub users                    |
| `/terminal-setup`            | Configure terminal for multiline input |
| `/theme [show\|set\|list]`   | Configure terminal theme               |

---

## Keyboard Shortcuts

### Global Shortcuts

| Shortcut | Action                                          |
| -------- | ----------------------------------------------- |
| `@`      | Mention files (include contents in context)     |
| `Esc`    | Cancel current operation                        |
| `!`      | Execute shell command directly (bypass Copilot) |
| `Ctrl+C` | Cancel operation / clear input / exit           |
| `Ctrl+D` | Shutdown                                        |
| `Ctrl+L` | Clear screen                                    |

### Timeline Shortcuts

| Shortcut | Action                            |
| -------- | --------------------------------- |
| `Ctrl+O` | Expand all / collapse timeline    |
| `Ctrl+R` | Expand recent / collapse timeline |

### Motion Shortcuts

| Shortcut   | Action                                  |
| ---------- | --------------------------------------- |
| `Ctrl+A`   | Move to beginning of line               |
| `Ctrl+E`   | Move to end of line                     |
| `Ctrl+H`   | Delete previous character               |
| `Ctrl+W`   | Delete previous word                    |
| `Ctrl+U`   | Delete from cursor to beginning of line |
| `Ctrl+K`   | Delete from cursor to end of line       |
| `Meta+←/→` | Move cursor by word                     |
| `↑ / ↓`    | Navigate command history                |

---

## MCP Server Configuration

### Managing MCP Servers

```bash
/mcp show              # List configured servers
/mcp add               # Interactive add (Tab to navigate, Ctrl+S to save)
/mcp edit <name>       # Modify existing server
/mcp delete <name>     # Remove server
/mcp disable <name>    # Disable without deleting
/mcp enable <name>     # Re-enable disabled server
```

### Configuration File

Location: `~/.copilot/mcp-config.json`
Override with: `XDG_CONFIG_HOME` environment variable

### JSON Schema

```json
{
  "mcpServers": {
    "server-name": {
      "command": "npx",
      "args": ["-y", "@package/mcp-server"],
      "env": {
        "MY_ENV_VAR": "my-value"
      }
    }
  }
}
```

### Built-in MCP Server

GitHub MCP server is pre-configured, providing:

- Repository access
- Issue and PR management
- Workflow run information
- GitHub API interactions

---

## Skills System

### Skill Locations

| Type     | Location                                    | Scope              |
| -------- | ------------------------------------------- | ------------------ |
| Project  | `.github/skills/` or `.claude/skills/`      | Current repository |
| Personal | `~/.copilot/skills/` or `~/.claude/skills/` | All projects       |

### SKILL.md Structure

```markdown
---
name: my-skill-name
description: What this skill does and when to use it
license: MIT (optional)
---

# Skill Title

## Purpose

What this skill accomplishes.

## Instructions

Step-by-step guidance for Copilot.
```

### Required Frontmatter

- **name**: Lowercase, hyphens allowed, unique identifier
- **description**: What skill does and when Copilot should use it

### Managing Skills

```bash
/skills list           # Show available skills
/skills info <name>    # Details about skill
/skills add <path>     # Add skill from path
/skills remove <name>  # Remove skill
/skills reload         # Refresh skill cache
```

---

## Custom Agents

### Agent Locations

| Type         | Location                               | Scope            |
| ------------ | -------------------------------------- | ---------------- |
| User         | `~/.copilot/agents/`                   | All projects     |
| Repository   | `.github/agents/`                      | Current repo     |
| Organization | `.github-private/agents/`              | Org repositories |
| Enterprise   | `.github-private/agents/` (enterprise) | Enterprise repos |

**Priority**: System > Repository > Organization > Enterprise

### Built-in Agents

| Agent       | Purpose                                                        |
| ----------- | -------------------------------------------------------------- |
| Explore     | Quick codebase analysis without adding to context              |
| Task        | Execute commands, brief success summaries, full failure output |
| Plan        | Create implementation plans before coding                      |
| Code-review | Review changes, surface only genuine issues                    |

### Using Agents

```bash
# Interactive selection
/agent

# In prompt
Use the refactoring agent to clean up this code

# Command line
copilot --agent=plan --prompt "Design authentication system"
```

---

## Custom Instructions

### Supported Files

| File                                        | Scope           | Description                     |
| ------------------------------------------- | --------------- | ------------------------------- |
| `.github/copilot-instructions.md`           | Repository-wide | General repository instructions |
| `.github/instructions/**/*.instructions.md` | Path-specific   | Instructions for specific paths |
| `AGENTS.md`                                 | Git root & cwd  | Agent behavior instructions     |
| `CLAUDE.md`                                 | Git root & cwd  | Claude-specific instructions    |
| `GEMINI.md`                                 | Git root & cwd  | Gemini-specific instructions    |
| `$HOME/.copilot/copilot-instructions.md`    | User-global     | Personal default instructions   |

### Environment Variable

`COPILOT_CUSTOM_INSTRUCTIONS_DIRS`: Additional directories for instructions

All custom instruction files are combined. None of them replaces another by priority.

---

## Permissions System

### Trusted Directories

On first launch in a directory, Copilot asks whether to trust it:

1. **Yes, proceed**: Trust for this session only
2. **Yes, and remember**: Trust for all future sessions
3. **No, exit**: Cancel the session

Remembered directories are kept in the `trusted_folders` array of `~/.copilot/config.json`, where they can be edited.

### Path Permissions

**Default Access**:

- Current working directory
- Subdirectories
- System temp directory

**Adding Directories**:

```bash
/add-dir /path/to/directory
```

**Bypass Path Verification**:

```bash
copilot --allow-all-paths
```

**Limitations**:

- Complex shell constructs may not detect paths
- Only `HOME`, `TMPDIR`, `PWD` and similar variables expanded
- Custom variables like `$MY_PROJECT_DIR` not validated
- Symlinks resolved for existing files only

### URL Permissions

**Default**: All URLs require approval

**Pre-approve Domains**:

```bash
copilot --allow-url github.com
```

**Bypass URL Verification**:

```bash
copilot --allow-all-urls
```

**Limitations**: URLs inside files and configuration are not detected, and obfuscated URLs may bypass the check.

### Tool Approval

When Copilot requests tool use:

1. **Yes**: Approve once
2. **Yes, and approve for session**: Approve for remaining session
3. **No (Esc)**: Deny and redirect

### Approval Flags

```bash
--allow-all-tools         # Allow all tools without approval
--allow-tool <spec>       # Allow a specific tool (repeatable)
--deny-tool <spec>        # Deny a specific tool (repeatable)
```

Tool specification formats:

- `'shell'`: all shell commands
- `'shell(git)'`: one command
- `'shell(git push)'`: a command with a subcommand
- `'write'`: file modification tools
- `'MCP_SERVER_NAME'`: all tools from an MCP server
- `'MCP_SERVER_NAME(tool_name)'`: one MCP tool

### Risk Mitigation

With `--allow-all-tools` in automated workflows:

- Run in a VM, a container or another isolated environment
- Restrict network access
- Deny dangerous commands with `--deny-tool`

---

## Environment Variables

| Variable                           | Description                                       |
| ---------------------------------- | ------------------------------------------------- |
| `GH_TOKEN`                         | GitHub authentication token (highest priority)    |
| `GITHUB_TOKEN`                     | GitHub authentication token                       |
| `XDG_CONFIG_HOME`                  | Override config directory (default: `~/.copilot`) |
| `COPILOT_CUSTOM_INSTRUCTIONS_DIRS` | Additional instruction directories                |

---

## Configuration Files

| File                      | Location            | Purpose                  |
| ------------------------- | ------------------- | ------------------------ |
| `config.json`             | `~/.copilot/`       | Trusted folders, theme   |
| `mcp-config.json`         | `~/.copilot/`       | MCP server configuration |
| `copilot-instructions.md` | `~/.copilot/`       | User-global instructions |
| `copilot-instructions.md` | `.github/`          | Repository instructions  |
| `SKILL.md`                | `.github/skills/*/` | Skill definitions        |

---

## Command Line Options

```bash
copilot [options]

Options:
  --banner              Show animated banner on launch
  --resume              Cycle through and resume sessions
  --continue            Resume most recent session
  --agent=<name>        Use specific agent
  -p, --prompt "<text>" Run a single prompt (programmatic mode)
  --allow-all-tools     Allow all tools without approval
  --allow-tool <spec>   Allow a specific tool (repeatable)
  --deny-tool <spec>    Deny a specific tool (repeatable)
  --allow-all-paths     Disable path verification
  --allow-all-urls      Disable URL verification
  --allow-url <domain>  Pre-approve specific domain
  --version             Show version
  --help                Show help
```

### Help Topics

```bash
copilot help              # General help
copilot help config       # Configuration settings
copilot help environment  # Environment variables
copilot help logging      # Logging levels
copilot help permissions  # Tool permissions
```

---

## Programmatic Mode

`-p` (or `--prompt`) runs one prompt and exits, for scripts and CI.

```bash
# Simple query
copilot -p "What does this repo do?"

# With one tool approved
copilot -p "Run the tests" --allow-tool 'shell(npm)'

# Full automation with dangerous commands denied
copilot -p "Fix linting errors and commit" \
  --allow-all-tools \
  --deny-tool 'shell(git push)' \
  --deny-tool 'shell(rm)'

# Piping input
echo "Explain this code" | copilot
cat script.sh | copilot -p "What does this do?"
```

See [Approval Flags](#approval-flags) for the tool specification formats.

---

## Updating and Version Management

```bash
copilot --version         # Check the installed version
```

- **Update**: Each installation method above gives its update command (`brew upgrade copilot-cli`, `npm update -g @github/copilot`, `winget upgrade GitHub.Copilot`). For the install script, run it again to install the latest version.
- **Specific version**: `npm install -g @github/copilot@<version>`, or set `VERSION` for the install script.
- **Prerelease**: Each installation method above gives its prerelease command.
- **Releases**: https://github.com/github/copilot-cli/releases/

---

## Troubleshooting

### "Copilot not found"

- Restart the terminal after installation
- Check that the install location is in `$PATH`:
  - Homebrew: `/opt/homebrew/bin` or `/usr/local/bin`
  - npm: the global npm bin directory
  - Install script: `$HOME/.local/bin` or `/usr/local/bin`
- Run `which copilot` (Unix) or `where copilot` (Windows)

### "Not logged in"

Run `/login` in interactive mode, or set `GH_TOKEN` or `GITHUB_TOKEN` (see [Authentication](#authentication)).

### "No subscription"

- Check the subscription at https://github.com/settings/copilot
- Make sure the plan includes CLI access
- Run `/logout`, then `/login`

### "Tool permission denied"

- Add the directory: `/add-dir /path`
- Or restart with `--allow-all-paths` or `--allow-all-urls`
- Check `trusted_folders` in `~/.copilot/config.json`
- Approve tools one by one when prompted

### "Context window full"

```bash
/context    # View token usage
/compact    # Summarise and free space
/clear      # Start fresh (loses context)
```

### MCP Server Errors

```bash
/mcp show            # Check status
/mcp edit <name>     # Fix configuration
/mcp disable <name>  # Disable for now
```

---

## Version Information

**Current Status**: Public Preview (with data protection)
**Repository**: https://github.com/github/copilot-cli
**Documentation**: https://docs.github.com/en/copilot/concepts/agents/about-copilot-cli

---

**Maintainer**: amplihack framework
**Last Review**: 2026-01-25
