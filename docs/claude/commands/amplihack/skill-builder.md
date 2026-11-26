---
name: amplihack:skill-builder
disable-model-invocation: true
version: 1.0.0
description: Build a new skill by handing the work to the skill-builder skill
argument-hint: <skill-name> <skill-type> <description>
triggers:
  - "create a new skill"
  - "build a Claude Code skill"
  - "generate skill for"
---

# Skill Builder Command

## Input Validation

@~/.amplihack/.claude/context/AGENT_INPUT_VALIDATION.md

## Usage

`/amplihack:skill-builder <skill-name> <skill-type> <description>`

**Arguments:**

- `skill-name`: Name of the skill (kebab-case, e.g., "data-transformer")
- `skill-type`: Type of skill - one of: `agent`, `command`, `scenario`, `skill`
- `description`: Brief description of what the skill does (1-2 sentences)

**Examples:**

```bash
/amplihack:skill-builder data-transformer skill "Transforms data between different formats with validation"
/amplihack:skill-builder analyze-dependencies command "Analyzes project dependencies and generates report"
/amplihack:skill-builder code-reviewer scenario "Reviews code for quality, security, and best practices"
/amplihack:skill-builder api-client agent "Manages API client connections with retry logic"
```

## How each type is built

- `skill`: build it with the `skill-builder` skill and follow that skill. It writes `.claude/skills/{skill_name}/SKILL.md`. That skill is the one statement of a skill's front matter, of the lengths of its name and description, and of the checklist a skill must pass. This command states none of them.
- `agent`: an agent definition, written to `.claude/agents/amplihack/specialized/{skill_name}.md`. It takes an agent's front matter, not a skill's.
- `command`: a slash command, written to `.claude/commands/amplihack/{skill_name}.md`. It takes a command's front matter, not a skill's.
- `scenario`: a production tool, described in `.claude/scenarios/{skill_name}/README.md`.
