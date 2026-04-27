<!-- Ported from upstream amplihack. Rust-specific adaptations applied where applicable. -->

# GitHub Copilot CLI Integration with amplihack

**Version**: 1.1.0
**Status**: Complete Integration (with Copilot CLI Transcript Support)
**Last Updated**: 2026-03-07

## Overview

This document describes the complete integration between GitHub Copilot CLI and the amplihack agentic coding framework. The integration provides Copilot users with access to amplihack's agents, skills, workflows, and MCP servers.

!!! note "Upstream Reference"
    This document was ported from the upstream Python amplihack project. Code
    examples showing Python hook implementations are preserved as upstream
    reference. The amplihack-rs Rust implementation may use different hook
    mechanisms. See [hook specifications](hook-specifications.md) for
    Rust-specific details.

**New in v1.1.0 (2026-03-07)**: Native Copilot CLI transcript support in Power-Steering checker. The checker now auto-detects and parses both Claude Code and GitHub Copilot CLI transcript formats (real `events.jsonl` format), enabling session completion validation across both platforms.

## Table of Contents

- [Copilot CLI Transcript Support](#copilot-cli-transcript-support)
- [Architecture](#architecture)
- [Quick Start](#quick-start)
- [Integration Components](#integration-components)
- [Usage Guide](#usage-guide)
- [Available Agents](#available-agents)
- [Available Skills](#available-skills)
- [Adaptive Hook System](#6-adaptive-hook-system)
- [MCP Servers](#mcp-servers)
- [Hooks and Automation](#hooks-and-automation)
- [Testing](#testing)
- [Troubleshooting](#troubleshooting)
- [Philosophy Alignment](#philosophy-alignment)

## Where Do Agents Come From?

Copilot CLI agents are **authored by the amplihack project** and stored in `~/.amplihack/.claude/agents/amplihack/`. When you run `amplihack copilot`, the framework symlinks these agents into `.github/agents/` so GitHub Copilot CLI can discover them. You don't need to write agents yourself — amplihack provides 30+ specialized agents (architect, builder, reviewer, tester, security, etc.) that Copilot can delegate to.

## Copilot CLI Transcript Support

**New in v1.1.0 (2026-03-07)**: Power-Steering now natively supports GitHub Copilot CLI session transcripts.

### What Changed

The `power_steering_checker` package has been refactored from a monolithic 5,063-line file into 12 focused modules with automatic transcript format detection:

- **Auto-Detection**: Automatically detects whether a transcript is from Claude Code or GitHub Copilot CLI
- **Copilot CLI Format**: Parses real `events.jsonl` format used by GitHub Copilot CLI
- **Backward Compatible**: All existing Claude Code transcripts continue to work
- **Tested**: 48 new parser tests + 22 Copilot CLI end-to-end tests (verified against 5 real Copilot sessions)

### How It Works

```
Session Transcript → Transcript Parser → Power-Steering Checker
                            ↓
                   Auto-detect format:
                   - Claude Code JSONL
                   - Copilot CLI events.jsonl
                            ↓
                   Parse appropriately
                            ↓
                   Validate session completion
```

### Key Features

1. **Format Auto-Detection**: No configuration needed - the parser detects the format automatically
2. **SDK Call Safety**: CLAUDECODE environment variable properly unset to prevent nested session errors
3. **Progress Tracking**: Works with both transcript formats for session completion validation
4. **Error Resilience**: Fail-open design ensures checker never blocks due to parsing errors

### Module Structure

The refactored checker is organized into specialized modules:

- `main_checker.py` — Orchestration + public API (1,217 lines, 76% reduction)
- `transcript_parser.py` — Format detection + parsing (both Claude Code and Copilot CLI)
- `session_detection.py` — Session type classification
- `considerations.py` — Check configuration + evaluation
- `sdk_calls.py` — Claude SDK integration + parallel analysis
- `progress_tracking.py` — State persistence + redirect records
- `result_formatting.py` — Output generation
- Plus 5 check-specific modules

### Testing

All Copilot CLI transcript support is thoroughly tested:

```bash
# Run parser tests (upstream Python)
pytest .claude/tools/amplihack/hooks/power_steering_checker/tests/test_transcript_parser.py

# Run Copilot CLI integration tests (upstream Python)
pytest .claude/tools/amplihack/hooks/tests/test_power_steering_copilot_cli.py

# Total test coverage
# - 121 existing tests (backward compatibility)
# - 48 parser tests (format detection + parsing)
# - 22 Copilot CLI e2e tests (real session validation)
# = 191 tests passing
```

### Benefits for Copilot CLI Users

1. **Session Completion Validation**: Power-Steering now works in Copilot CLI sessions
2. **Quality Enforcement**: Same 21 considerations apply (TODOs, tests, CI, PR quality, etc.)
3. **No Configuration**: Auto-detection means zero setup required
4. **Cross-Platform**: Same checker logic works in both Claude Code and Copilot CLI

### Migration Notes

**No action required** - if you're already using amplihack with Copilot CLI, the transcript support is automatically enabled. The checker will:

1. Detect you're using Copilot CLI (via transcript format)
2. Parse the `events.jsonl` format correctly
3. Apply the same 21 considerations as Claude Code
4. Provide session completion validation

## Architecture

### Directory Structure

```
.github/
├── agents/                      # GitHub Copilot agents
│   ├── amplihack/ -> ../../.claude/agents/amplihack/  (symlink)
│   ├── skills/ -> ../../.claude/skills/               (symlinks)
│   ├── README.md                # Agent documentation
│   └── REGISTRY.json            # Agent registry
├── commands/                    # Converted slash commands
│   └── [command-name].md        # Command documentation
├── copilot-instructions.md      # Base Copilot instructions
├── hooks/                       # Git and session hooks
│   ├── pre-commit               # Pre-commit validation
│   ├── post-checkout            # Post-checkout setup
│   ├── session-start            # Session initialization
│   └── [other hooks]            # Additional hooks
└── mcp-servers.json             # MCP server configuration

.claude/
├── agents/amplihack/            # Source of truth for agents
│   ├── core/                    # Core agents (architect, builder, etc.)
│   ├── specialized/             # Specialized agents
│   └── workflows/               # Workflow agents
├── skills/                      # Source of truth for skills
│   ├── [skill-name]/            # Individual skills
│   └── README.md                # Skills documentation
└── commands/amplihack/          # Source of truth for commands
    └── [command-name].md        # Command implementations
```

### Key Principles

1. **Source of Truth**: All content lives in `~/.amplihack/.claude/` directory
2. **Symlinks for Access**: `.github/` uses symlinks to `~/.amplihack/.claude/` content
3. **No Duplication**: Single source of truth prevents drift
4. **Safe for Build Tools**: Symlinks use `followlinks=True` in build scripts
5. **Philosophy Aligned**: Ruthless simplicity, no complex sync systems

### Symlink Architecture

**CORRECT Pattern** (What We Use):

```
.claude/agents/amplihack/          ← REAL FILES (source)
.github/agents/amplihack/          ← SYMLINK to ../../.claude/agents/amplihack/

.claude/skills/[skill-name]/       ← REAL DIRECTORIES (source)
.github/agents/skills/[skill-name] ← SYMLINK to ../../../.claude/skills/[skill-name]
```

**Why This Works**:

- Build tools (`build_hooks.py`) can process with `followlinks=True`
- No circular symlinks
- Single source of truth
- Changes in `~/.amplihack/.claude/` automatically available in `.github/`

**INCORRECT Pattern** (What Breaks):

```
.claude/agents/amplihack/ ← symlink
.github/agents/amplihack/ ← symlink
(Both pointing to each other or to same target = circular reference or build breaks)
```

## Quick Start

### Installation

1. **Install GitHub Copilot CLI** (if not already installed):

   ```bash
   gh extension install github/gh-copilot
   ```

2. **Verify Integration**:

   ```bash
   # Check agents are accessible
   ls -la .github/agents/amplihack/

   # Check skills are accessible
   ls -la .github/agents/skills/

   # Verify MCP servers
   cat .github/mcp-servers.json
   ```

3. **Test Basic Functionality**:

   ```bash
   # Use Copilot with amplihack context
   gh copilot explain .github/copilot-instructions.md

   # Get suggestions following amplihack philosophy
   gh copilot suggest "create a new module following brick pattern"
   ```

### First Steps

1. Read the base instructions:

   ```bash
   gh copilot explain .github/copilot-instructions.md
   ```

2. Understand available agents:

   ```bash
   gh copilot explain .github/agents/README.md
   ```

3. Explore available skills:
   ```bash
   gh copilot explain .github/agents/skills/README.md
   ```

## Integration Components

### 1. Base Instructions File

**File**: `.github/copilot-instructions.md`

**Purpose**: Provides core amplihack philosophy and patterns to GitHub Copilot

**Key Sections**:

- Core Philosophy (Zen of Simple Code, Brick Philosophy)
- Architecture Overview
- User Preferences and Autonomy Guidelines
- Testing Strategy
- Common Patterns
- Getting Started Guide

**Usage**:

```bash
# Copilot automatically loads this file when working in the repo
# You can also reference it explicitly:
gh copilot suggest --context .github/copilot-instructions.md "your task"
```

### 2. Agents Directory

**Structure**:

```
.github/agents/
├── amplihack/ -> ../../.claude/agents/amplihack/  (symlink to all agents)
├── skills/ -> ../../.claude/skills/               (symlinks to skills)
├── README.md                                      (agent documentation)
└── REGISTRY.json                                  (agent registry)
```

**Available Agents**:

#### Core Agents (in `.github/agents/amplihack/core/`)

- **architect.md**: Solution design and architecture
- **builder.md**: Code implementation from specs
- **reviewer.md**: Code review and quality checks
- **tester.md**: Test generation and validation
- **optimizer.md**: Performance improvements
- **api-designer.md**: API contract design

#### Specialized Agents (in `.github/agents/amplihack/specialized/`)

- **analyzer.md**: Deep code analysis
- **cleanup.md**: Code simplification
- **ambiguity.md**: Requirement clarification
- **fix-agent.md**: Rapid error resolution
- **ci-diagnostic-workflow.md**: CI failure diagnosis
- **pre-commit-diagnostic.md**: Pre-commit hook issues
- **knowledge-archaeologist.md**: Deep investigation
- And many more...

**Usage**:

```bash
# Reference an agent in your query
gh copilot suggest -a .github/agents/amplihack/core/architect.md \
  "design authentication system"

# Or reference in context
gh copilot suggest --context .github/agents/amplihack/core/builder.md \
  "implement the authentication module"
```

### 3. Skills Directory

**Structure**:

```
.github/agents/skills/
├── [skill-name]/ -> ../../../.claude/skills/[skill-name]/  (symlinks)
├── README.md                                               (skills documentation)
└── SKILLS_REGISTRY.json                                    (skills registry)
```

**Available Skills** (70+ skills):

#### Development Skills

- **agent-sdk**: Agent SDK architecture and patterns
- **code-smell-detector**: Anti-pattern detection
- **design-patterns-expert**: GoF design patterns
- **documentation-writing**: Clear documentation
- **module-spec-generator**: Module specification generation
- **outside-in-testing**: Agentic testing framework

#### Workflow Skills

- **default-workflow**: Standard development workflow
- **investigation-workflow**: Deep system analysis
- **cascade-workflow**: Graceful degradation
- **n-version-workflow**: N-version programming
- **debate-workflow**: Multi-agent debate
- **consensus-voting**: Consensus decision making

#### Domain Expert Skills (30+ analyst skills)

- **architect-analyst**: Architecture analysis
- **security-analyst**: Security review
- **performance-analyst**: Performance optimization
- **economist-analyst**: Economic impact analysis
- **historian-analyst**: Historical context analysis
- And 25+ more domain experts...

#### Collaboration Skills

- **email-drafter**: Professional email generation
- **meeting-synthesizer**: Meeting notes processing
- **knowledge-extractor**: Learning capture
- **mermaid-diagram-generator**: Architecture diagrams

**Usage**:

```bash
# Use a skill in your query
gh copilot suggest --context .github/agents/skills/code-smell-detector/ \
  "review this code for anti-patterns"

# Reference multiple skills
gh copilot suggest \
  --context .github/agents/skills/architect-analyst/ \
  --context .github/agents/skills/security-analyst/ \
  "design a secure authentication system"
```

### 4. Commands Directory

**Structure**:

```
.github/commands/
└── [command-name].md           # Converted slash command documentation
```

**Purpose**: Converts Claude Code slash commands (e.g., `/dev`, `/analyze`) into documentation that Copilot can reference.

**Available Commands**:

- **dev**: Multi-agent orchestration
- **analyze**: Codebase analysis
- **improve**: Self-improvement workflow
- **fix**: Intelligent fix dispatch
- **ddd** (Document-Driven Development): Phases 0-5
- **customize**: User preference management
- **n-version**: N-version programming
- **debate**: Multi-agent debate
- **cascade**: Fallback cascade

**Usage**:

```bash
# Reference a command's approach
gh copilot explain .github/commands/dev.md

# Use command pattern in suggestions
gh copilot suggest --context .github/commands/analyze.md \
  "analyze the codebase for patterns"
```

### 5. Hooks

**Structure**:

```
.github/hooks/
├── pre-commit               # Bash wrapper -> Python implementation
├── post-checkout            # Bash wrapper -> Python implementation
├── session-start            # Bash wrapper -> Python implementation
├── pre-push                 # Bash wrapper -> Python implementation
├── commit-msg               # Bash wrapper -> Python implementation
└── post-merge               # Bash wrapper -> Python implementation
```

**Hook Types**:

#### Git Hooks

- **pre-commit**: Linting, formatting, type checking
- **commit-msg**: Commit message validation
- **pre-push**: Run tests before push
- **post-checkout**: Setup after branch switch
- **post-merge**: Cleanup after merge

#### Session Hooks

- **session-start**: Initialize session, check version, load preferences
- **session-end**: Cleanup, Neo4j shutdown (if applicable)

**Implementation Pattern**:

Each hook is a **bash wrapper** that calls a **Python implementation**:

!!! note "Upstream Python hook pattern"
    The bash+Python hook implementation below describes the **upstream Python
    amplihack** pattern. amplihack-rs uses Rust-native hooks where applicable.
    See [hook specifications](hook-specifications.md) and
    [configure hooks](../howto/configure-hooks.md) for the Rust approach.

**Bash Wrapper** (`.github/hooks/pre-commit`):

```bash
#!/usr/bin/env bash
# GitHub Copilot compatible pre-commit hook
# Calls Python implementation in .claude/tools/amplihack/hooks/

python3 .claude/tools/amplihack/hooks/pre_commit.py "$@"
```

**Python Implementation** (`~/.amplihack/.claude/tools/amplihack/hooks/pre_commit.py`):

```python
#!/usr/bin/env python3
"""Pre-commit hook implementation."""

def main():
    # Implementation here
    pass

if __name__ == "__main__":
    main()
```

**Why This Pattern**:

1. Bash wrappers are simple and GitHub Copilot compatible
2. Python implementations can be complex and tested
3. Clear separation of concerns
4. Easy to maintain and debug

### 6. Adaptive Hook System

**Challenge**: Claude Code and GitHub Copilot CLI have different hook capabilities.

**Solution**: amplihack uses an adaptive hook system that detects which platform is calling and applies appropriate strategies for context injection.

#### Platform Detection

The hook system automatically detects the calling platform by checking:

1. Environment variables (`CLAUDE_CODE`, `GITHUB_COPILOT`)
2. Process name patterns
3. Fallback to Claude Code behavior (safe default)

#### Context Injection Strategies

| Platform        | Strategy             | Method                                                         |
| --------------- | -------------------- | -------------------------------------------------------------- |
| **Claude Code** | Direct injection     | `hookSpecificOutput.additionalContext` or stdout               |
| **Copilot CLI** | File-based injection | Write to `.github/agents/AGENTS.md` with `@include` directives |

**Claude Code** (Direct Injection):

```python
# Hook returns JSON with context
return {
    "hookSpecificOutput": {
        "additionalContext": "User preferences: talk like a pirate"
    }
}
# AI sees context immediately
```

**Copilot CLI** (File-Based Injection):

```python
# Hook writes to AGENTS.md
with open(".github/agents/AGENTS.md", "w") as f:
    f.write("""
# Active Agents and Context

@~/.amplihack/.claude/context/USER_PREFERENCES.md
@~/.amplihack/.claude/context/PHILOSOPHY.md
    """)
# Copilot reads file via @include on next request
```

#### Why This Workaround is Needed

**Copilot CLI Limitation**: Hook output is ignored for context injection (except `preToolUse` permission decisions). See the [hooks comparison](../concepts/hooks-comparison.md) for detailed comparison.

**Our Solution Benefits**:

- Preference injection works on both platforms
- Context loading works everywhere
- Zero duplication (single Python implementation)
- Automatic platform adaptation

**What Works Where**:

| Feature | Claude Code | Copilot CLI | Implementation |
|---------|-------------|-------------|----------------|
| Logging | Direct | Direct | Same hooks |
| Blocking tools | preToolUse | preToolUse | Same hooks |
| Context injection | hookOutput | AGENTS.md | Adaptive strategy |
| Preferences | hookOutput | AGENTS.md | Adaptive strategy |

For complete hook capability comparison, see [hooks comparison](../concepts/hooks-comparison.md).

### 7. MCP Servers

**File**: `.github/mcp-servers.json`

**Purpose**: Configures Model Context Protocol (MCP) servers for GitHub Copilot to access filesystem, git, and other services.

**Default Configuration**:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/project"],
      "env": {},
      "disabled": false
    },
    "git": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-git", "--repository", "/path/to/project"],
      "env": {},
      "disabled": false
    }
  }
}
```

**Available MCP Servers**:

- **filesystem**: Safe file operations with path restrictions
- **git**: Git operations (status, diff, log, etc.)
- **github**: GitHub API access (issues, PRs, etc.)
- **docker**: Docker container management (if installed)

**Security**:

- Filesystem server restricted to project directory
- Git server restricted to current repository
- No destructive operations without confirmation
- Environment variable isolation

**Usage**:

```bash
# MCP servers are automatically loaded by GitHub Copilot
# when mcp-servers.json is present in .github/

# You can also manually start MCP servers:
npx -y @modelcontextprotocol/server-filesystem /path/to/project
```

## Usage Guide

### Basic Workflow

1. **Start with Context**:

   ```bash
   # Load amplihack philosophy
   gh copilot explain .github/copilot-instructions.md
   ```

2. **Use Agents for Guidance**:

   ```bash
   # Get architectural guidance
   gh copilot suggest -a .github/agents/amplihack/core/architect.md \
     "design a REST API for user authentication"
   ```

3. **Reference Patterns**:

   ```bash
   # Check for existing patterns
   gh copilot explain .claude/context/PATTERNS.md

   # Use pattern in implementation
   gh copilot suggest --context .claude/context/PATTERNS.md \
     "implement safe subprocess wrapper"
   ```

4. **Implement with Builder**:

   ```bash
   # Generate implementation
   gh copilot suggest -a .github/agents/amplihack/core/builder.md \
     "implement authentication API from spec"
   ```

5. **Review with Reviewer**:
   ```bash
   # Review code for philosophy compliance
   gh copilot explain --review src/auth/ \
     --context .github/agents/amplihack/core/reviewer.md
   ```

### Advanced Patterns

#### Multi-Agent Consultation

```bash
# Consult multiple agents
gh copilot suggest \
  -a .github/agents/amplihack/core/architect.md \
  -a .github/agents/amplihack/specialized/security.md \
  -a .github/agents/amplihack/specialized/database.md \
  "design secure user authentication with database storage"
```

#### Pattern-Driven Development

```bash
# Reference specific pattern
gh copilot suggest \
  --context .claude/context/PATTERNS.md \
  --context .github/agents/amplihack/core/builder.md \
  "implement module following Bricks & Studs pattern"
```

#### Philosophy-Guided Review

```bash
# Review for philosophy compliance
gh copilot explain --review src/module/ \
  --context .claude/context/PHILOSOPHY.md \
  --context .github/agents/amplihack/core/reviewer.md
```

### Integration with Claude Code

GitHub Copilot CLI and Claude Code can work together:

1. **Use Claude Code for Workflows**: Complex multi-step workflows
2. **Use Copilot for Quick Suggestions**: Rapid code generation
3. **Share Context**: Both use same `~/.amplihack/.claude/context/` files
4. **Complementary Tools**: Different strengths, same philosophy

**Example Workflow**:

```bash
# 1. Claude Code for high-level design
# (In Claude Code)
/dev "design authentication system"

# 2. Copilot for implementation
gh copilot suggest -a .github/agents/amplihack/core/builder.md \
  "implement JWT token validation"

# 3. Claude Code for review and testing
# (In Claude Code)
/amplihack:analyze src/auth/
```

## Available Agents

### Core Development Agents

| Agent            | Purpose                          | When to Use                             |
| ---------------- | -------------------------------- | --------------------------------------- |
| **architect**    | Solution design and architecture | Designing new features, system redesign |
| **builder**      | Code implementation from specs   | Implementing features, writing code     |
| **reviewer**     | Code review and quality checks   | Before commits, PR reviews              |
| **tester**       | Test generation and validation   | Writing tests, TDD                      |
| **optimizer**    | Performance improvements         | Bottleneck analysis, optimization       |
| **api-designer** | API contract design              | Designing APIs, defining interfaces     |

### Specialized Agents

| Agent                       | Purpose                   | When to Use                             |
| --------------------------- | ------------------------- | --------------------------------------- |
| **analyzer**                | Deep code analysis        | Understanding complex code, refactoring |
| **cleanup**                 | Code simplification       | Removing complexity, simplifying code   |
| **ambiguity**               | Requirement clarification | Unclear requirements, edge cases        |
| **fix-agent**               | Rapid error resolution    | Quick fixes, common error patterns      |
| **ci-diagnostic**           | CI failure diagnosis      | CI failures, build issues               |
| **pre-commit-diagnostic**   | Pre-commit hook issues    | Pre-commit failures, formatting issues  |
| **knowledge-archaeologist** | Deep investigation        | Understanding legacy code, research     |

### Workflow Agents

| Agent                    | Purpose                  | When to Use                                    |
| ------------------------ | ------------------------ | ---------------------------------------------- |
| **prompt-writer**        | Task clarification       | Clarifying requirements, defining scope        |
| **documentation-writer** | Documentation generation | Writing docs, API documentation                |
| **philosophy-guardian**  | Philosophy compliance    | Ensuring simplicity, catching over-engineering |
| **worktree-manager**     | Git worktree operations  | Managing multiple branches, parallel work      |

## Available Skills

### Development Skills (20+)

- **agent-sdk**: Comprehensive Agent SDK knowledge
- **code-smell-detector**: Identifies anti-patterns
- **design-patterns-expert**: GoF design patterns
- **documentation-writing**: Clear documentation following Eight Rules
- **module-spec-generator**: Generates module specifications
- **outside-in-testing**: Agentic testing framework
- **goal-seeking-agent-pattern**: When to use autonomous agents

### Workflow Skills (6)

- **default-workflow**: Standard 22-step development workflow
- **investigation-workflow**: 6-phase investigation workflow
- **cascade-workflow**: Graceful degradation patterns
- **n-version-workflow**: N-version programming
- **debate-workflow**: Multi-agent debate
- **consensus-voting**: Consensus decision making

### Domain Expert Skills (30+)

#### STEM Analysts

- **computer-scientist-analyst**: Computational complexity, algorithms
- **engineer-analyst**: Technical systems, first principles
- **physicist-analyst**: Physics-based analysis
- **chemist-analyst**: Chemistry lens analysis
- **biologist-analyst**: Biological systems analysis
- **cybersecurity-analyst**: Security, threat modeling

#### Social Science Analysts

- **economist-analyst**: Economic impact, incentives
- **psychologist-analyst**: Human behavior, UX
- **sociologist-analyst**: Social systems, culture
- **anthropologist-analyst**: Cultural analysis
- **political-scientist-analyst**: Governance, policy
- **historian-analyst**: Historical patterns, precedents

#### Humanities Analysts

- **philosopher-analyst**: Logic, ethics, reasoning
- **novelist-analyst**: Narrative analysis
- **poet-analyst**: Creative expression
- **journalist-analyst**: Fact-checking, investigation
- **lawyer-analyst**: Legal analysis, compliance
- **ethicist-analyst**: Moral reasoning, ethics

#### Specialized Analysts

- **futurist-analyst**: Scenario planning, trends
- **urban-planner-analyst**: System design, infrastructure
- **environmentalist-analyst**: Sustainability, ecology
- **epidemiologist-analyst**: Disease patterns, health
- **indigenous-leader-analyst**: Indigenous knowledge systems

### Collaboration Skills (10+)

- **email-drafter**: Professional email generation
- **meeting-synthesizer**: Meeting notes processing
- **knowledge-extractor**: Learning capture
- **mermaid-diagram-generator**: Architecture diagrams
- **learning-path-builder**: Technology onboarding
- **work-delegator**: Task distribution
- **workstream-coordinator**: Parallel workflow management
- **storytelling-synthesizer**: Narrative generation

### Utility Skills (10+)

- **context_management**: Token monitoring, context optimization
- **skill-builder**: Creating new skills
- **mcp-manager**: MCP server configuration
- **backlog-curator**: Backlog management
- **roadmap-strategist**: Roadmap planning
- **test-gap-analyzer**: Test coverage analysis
- **pr-review-assistant**: PR review automation

## MCP Servers

### Filesystem Server

**Purpose**: Safe file operations with path restrictions

**Capabilities**:

- Read files
- Write files
- List directories
- Search files
- Create directories

**Security**:

- Restricted to project directory
- No access outside allowed paths
- Safe deletion with confirmations

**Configuration**:

```json
{
  "filesystem": {
    "command": "npx",
    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/project"],
    "env": {},
    "disabled": false
  }
}
```

### Git Server

**Purpose**: Git operations without shell access

**Capabilities**:

- `git status`
- `git diff`
- `git log`
- `git add`
- `git commit`
- `git branch`
- No destructive operations (reset, force push, etc.)

**Security**:

- Restricted to current repository
- No force operations
- Commit requires message
- No remote operations without confirmation

**Configuration**:

```json
{
  "git": {
    "command": "npx",
    "args": ["-y", "@modelcontextprotocol/server-git", "--repository", "/path/to/project"],
    "env": {},
    "disabled": false
  }
}
```

### GitHub Server (Optional)

**Purpose**: GitHub API access for issues, PRs, etc.

**Capabilities**:

- Create issues
- List PRs
- Comment on PRs
- Check CI status
- No merge operations

**Security**:

- Requires GitHub token
- Read-only by default
- No destructive operations

**Configuration**:

```json
{
  "github": {
    "command": "npx",
    "args": ["-y", "@modelcontextprotocol/server-github"],
    "env": {
      "GITHUB_TOKEN": "${GITHUB_TOKEN}"
    },
    "disabled": false
  }
}
```

### Docker Server (Optional)

**Purpose**: Docker container management

**Capabilities**:

- List containers
- Start/stop containers
- View logs
- No volume mounts outside project

**Security**:

- Restricted to project containers
- No privileged mode
- No host network access

**Configuration**:

```json
{
  "docker": {
    "command": "npx",
    "args": ["-y", "@modelcontextprotocol/server-docker"],
    "env": {},
    "disabled": true
  }
}
```

## Hooks and Automation

**Note**: For information about how hooks adapt to different platforms (Claude Code vs Copilot CLI), see [Adaptive Hook System](#6-adaptive-hook-system).

### Git Hooks

All git hooks follow the bash wrapper pattern described in the upstream Python implementation:

#### Pre-Commit Hook

**Purpose**: Run linting, formatting, and type checking before commit

**Checks**:

- Linting (ruff, pylint, clippy, etc.)
- Formatting (black, prettier, rustfmt, etc.)
- Type checking (mypy, pyright, etc.)
- Test runs (optional)

#### Commit-Msg Hook

**Purpose**: Validate commit message format

**Validation**:

- Conventional commits format
- Maximum line length
- Issue reference (optional)

#### Pre-Push Hook

**Purpose**: Run tests before pushing

**Checks**:

- Unit tests
- Integration tests (optional)
- Coverage threshold (optional)

#### Post-Checkout Hook

**Purpose**: Setup environment after branch switch

**Actions**:

- Install dependencies (if needed)
- Clear caches (if needed)
- Update submodules (if any)

#### Post-Merge Hook

**Purpose**: Cleanup after merge

**Actions**:

- Remove merged branches
- Update dependencies (if changed)
- Clear caches (if needed)

### Session Hooks

#### Session-Start Hook

**Purpose**: Initialize session, check version, load preferences

**Actions**:

- Check amplihack version
- Load user preferences
- Initialize logging
- Check Neo4j status (if used)

#### Session-End Hook

**Purpose**: Cleanup on session end

**Actions**:

- Save session logs
- Shutdown Neo4j (if auto_shutdown enabled)
- Clear temporary files

### Hook Installation

```bash
# Using amplihack-rs
amplihack install

# Or configure hooks manually — see the howto guide:
# docs/howto/configure-hooks.md
```

## Testing

!!! note "Upstream Python test commands"
    The `pytest` commands in the Copilot CLI Transcript Support section above
    reference the upstream Python test suite. For amplihack-rs, use `cargo test`
    to run the Rust test suite.

### Running Tests

```bash
# Run all tests (amplihack-rs)
cargo test

# Run specific test suite
cargo test --test integration

# Run with coverage
cargo llvm-cov
```

### Test Structure

```
tests/
├── unit/                   # Unit tests (60%)
│   ├── test_hooks.rs
│   ├── test_agents.rs
│   └── test_skills.rs
├── integration/            # Integration tests (30%)
│   ├── test_copilot_integration.rs
│   ├── test_mcp_servers.rs
│   └── test_hooks_workflow.rs
└── e2e/                    # End-to-end tests (10%)
    ├── test_full_workflow.rs
    └── test_copilot_usage.rs
```

## Troubleshooting

### Common Issues

#### Symlinks Not Working

**Problem**: Symlinks don't resolve correctly

**Solution**:

```bash
# Verify symlinks exist
ls -la .github/agents/amplihack
ls -la .github/agents/skills/

# Recreate symlinks if needed
cd .github/agents/
rm amplihack
ln -s ../../.claude/agents/amplihack amplihack

# For skills
cd skills/
rm [skill-name]
ln -s ../../../.claude/skills/[skill-name] [skill-name]
```

#### Hooks Not Executing

**Problem**: Git hooks don't run

**Solution**:

```bash
# Check hook permissions
ls -la .git/hooks/

# Make hooks executable
chmod +x .github/hooks/*

# Reinstall hooks
amplihack install
```

#### MCP Servers Not Starting

**Problem**: MCP servers fail to start

**Solution**:

```bash
# Check npx is installed
npx --version

# Test MCP server manually
npx -y @modelcontextprotocol/server-filesystem /path/to/project

# Check configuration
cat .github/mcp-servers.json

# Verify paths are absolute
sed -i 's|/path/to/project|'$(pwd)'|g' .github/mcp-servers.json
```

#### Copilot Not Finding Agents

**Problem**: GitHub Copilot doesn't recognize agents

**Solution**:

```bash
# Verify agents directory exists
ls -la .github/agents/

# Check symlinks are valid
file .github/agents/amplihack
file .github/agents/skills/[skill-name]

# Verify Copilot can read files
gh copilot explain .github/agents/README.md
```

### Debug Mode

Enable debug logging:

```bash
# Enable debug for hooks
export AMPLIHACK_DEBUG=1
bash .github/hooks/pre-commit

# Enable debug for MCP servers
export MCP_DEBUG=1
npx -y @modelcontextprotocol/server-filesystem $(pwd)

# View logs
tail -f .claude/runtime/logs/debug.log
```

## Philosophy Alignment

### Ruthless Simplicity

**Applied to Integration**:

- Single source of truth (`~/.amplihack/.claude/`)
- Symlinks instead of duplication
- Bash wrappers for hooks (simple)
- Python implementations (testable)
- No complex sync systems

**What We Avoid**:

- Duplicating files between `~/.amplihack/.claude/` and `.github/`
- Complex synchronization scripts
- Circular symlinks
- Over-engineered hook systems

### Zero-BS Implementation

**Applied to Integration**:

- All hooks actually work (no stubs)
- All agents are functional
- All MCP servers are configured correctly
- No placeholder content

### Modular Design

**Applied to Integration**:

- Hooks are self-contained
- MCP servers are independent
- Agents are modular
- Skills are isolated

### Testing Strategy

**Applied to Integration**:

- 60% unit tests (hook wrappers, configs)
- 30% integration tests (hook workflows, MCP servers)
- 10% E2E tests (full workflows)

## Additional Resources

### Documentation

- **GitHub Copilot CLI**: <https://docs.github.com/en/copilot/github-copilot-in-the-cli>
- **MCP Servers**: <https://modelcontextprotocol.io/>
- **amplihack Philosophy**: [Philosophy](../concepts/philosophy.md)
- **amplihack Patterns**: [Patterns](../concepts/patterns.md)

### Support

- **Issues**: File issues in amplihack repository
- **Discussions**: Use GitHub Discussions
- **Documentation**: Check docs/ directory

### Contributing

See the project CONTRIBUTING guidelines for contribution details.

---

**Version**: 1.1.0
**Framework**: amplihack - Agentic coding framework
**Philosophy**: Ruthless simplicity + Modular design + AI-regeneratable architecture
