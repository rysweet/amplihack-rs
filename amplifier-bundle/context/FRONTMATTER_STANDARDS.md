# Frontmatter Standards

**Version**: 3.0.0
**Status**: Active

## Purpose

YAML frontmatter provides metadata for skills, commands, workflows, and agents.
This document aligns with the **Agent Skills open standard**
([agentskills.io/specification](https://agentskills.io/specification)),
**Anthropic skill authoring best practices**
([platform.claude.com](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices)),
and **Claude Code extensions**
([docs.claude.com/en/docs/claude-code/skills](https://docs.claude.com/en/docs/claude-code/skills)).

## Skills (Agent Skills Standard)

Skills follow the [Agent Skills specification](https://agentskills.io/specification).
Claude Code adds optional extensions for invocation control and subagent execution.

### Required Fields

```yaml
---
name: skill-name
description: What this skill does and when to use it.
---
```

| Field         | Required | Constraints |
| ------------- | -------- | ----------- |
| `name`        | **Yes**  | Max 64 chars. Lowercase letters, numbers, hyphens only. Must match directory name. |
| `description` | **Yes**  | Spec maximum is 1024 chars, but **target ~120** — see [Description Length](#description-length). Single-line scalar. Include the trigger keywords that help Claude decide when to auto-load the skill. |

### Optional Fields (Agent Skills Spec)

| Field           | Description |
| --------------- | ----------- |
| `license`       | License name or reference to a bundled license file. |
| `compatibility` | Environment requirements (max 500 chars). E.g., "Requires git and Rust 1.85+". |
| `metadata`      | Arbitrary key-value map. Use for version, author, or other custom data. |
| `allowed-tools` | Space-delimited list of pre-approved tools (experimental). |

### Optional Fields (Claude Code Extensions)

| Field                      | Description |
| -------------------------- | ----------- |
| `disable-model-invocation` | `true` to prevent Claude from auto-loading. Use for manual-only workflows like `/deploy`. |
| `user-invocable`           | `false` to hide from the `/` menu. Use for background knowledge. |
| `model`                    | Model override when this skill is active. |
| `context`                  | `fork` to run in a forked subagent context. |
| `agent`                    | Subagent type when `context: fork` is set (e.g., `Explore`, `Plan`). |
| `hooks`                    | Hooks scoped to this skill's lifecycle. |
| `argument-hint`            | Hint shown during autocomplete (e.g., `[issue-number]`). |

### Complete Example

```yaml
---
name: quality-audit
description: Iterative codebase quality audit with multi-agent validation. Use when auditing code quality, technical debt, or architecture.
metadata:
  version: "3.0"
  author: amplihack
---
```

### Description Length

Every bundled skill's `name` + `description` is resident in **every** Claude
Code session. That listing has a size budget, and past it Claude Code silently
truncates the tail to bare names — a truncated skill still installs, still runs
when invoked by name, and can no longer be matched to a task automatically.

The rules, enforced by `scripts/check-skill-description-budget.sh`, a pre-commit
hook, and an install-time check:

| Rule | Why |
| ---- | --- |
| Target **~120 characters** | 130 skills x ~120 chars keeps the whole listing under 18,000, comfortably inside the 20,000-character limit. |
| Use the form `<What it does>. Use when <triggers>.` | The trigger wording is what the matcher keys on. Taxonomy prose ("Provides insights on…", "Evaluates:…") costs characters and matches nothing. |
| **Single-line scalar** — no `\|` or `>` block form | At ~120 chars a block scalar buys nothing and makes the diff harder to check. The budget script rejects block scalars outright. |
| Quote only when the value contains a `:` or starts with a YAML indicator | Keeps the common case unquoted and readable. |
| No imperatives aimed at the agent, no URLs, no paths, no commands | These strings are injected into an agent's context by construction. |

The spec's 1024-character maximum is a per-field ceiling, not a target. A single
1024-character description would consume 5% of the entire listing budget.

See [Skill Listing Budget](../../docs/reference/skill-listing-budget.md) for the
constant and the API, and
[Keep Skill Descriptions in Budget](../../docs/howto/keep-skill-descriptions-in-budget.md)
for how to measure and fix.

### What NOT to Put in Skill Frontmatter

These fields were used in earlier versions of this doc but are **not recognized**
by Claude Code or the Agent Skills spec. They are silently ignored:

- ~~`auto_activates`~~ — Put activation keywords in the `description` trigger clause instead.
- ~~`priority_score`~~ — Not a real field. Claude uses `description` quality.
- ~~`evaluation_criteria`~~ — Not a real field.
- ~~`version`~~ — Use `metadata.version` if needed.
- ~~`invokes`~~ — Not a real field. Document dependencies in the markdown body.
- ~~`philosophy`~~ — Not a real field. Document alignment in the markdown body.
- ~~`maturity`~~ — Not a real field. Use `metadata.maturity` if needed.
- ~~`source_urls`~~ — Not a real field.

> **Migration note**: Many existing skills still use deprecated fields. They
> are harmless (silently ignored) but should be cleaned up over time. New skills
> must use only the fields listed above.

## Commands (Amplihack-Specific)

Commands are amplihack's user-invokable entry points via `/command-name`.
These are not part of the Agent Skills standard but follow similar conventions.

### Recommended Fields

```yaml
---
name: command-name
description: One-line summary (under 80 chars)
---
```

Additional fields like `triggers`, `invokes`, and `dependencies` can be
documented in the markdown body rather than frontmatter, since Claude Code
does not parse custom frontmatter fields.

## Workflows (Amplihack-Specific)

Workflows are multi-step process templates. They are amplihack constructs,
not part of the Agent Skills standard.

### Recommended Fields

```yaml
---
name: WORKFLOW_NAME
description: What this workflow orchestrates
steps: 23
---
```

The `steps` field is used by amplihack tooling for validation. Other metadata
(phases, entry points, references) belongs in the markdown body.

## Subagents (Claude Code Feature)

Subagents are defined in `.claude/agents/` and follow Claude Code's agent
format. See [Claude Code subagents documentation](https://docs.claude.com/en/docs/claude-code/sub-agents)
for the official specification.

## Naming Conventions

- **Skills**: `kebab-case` (directory: `skill-name/`, file: `SKILL.md`)
- **Commands**: `kebab-case` (file: `command-name.md`, invoke: `/command-name`)
- **Workflows**: `SCREAMING_SNAKE_CASE` (file: `WORKFLOW_NAME.md`)
- **Subagents**: `kebab-case` (file: `agent-name.md`)

## References

- **Agent Skills Specification**: https://agentskills.io/specification
- **Skill Authoring Best Practices**: https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices
- **Claude Code Skills Documentation**: https://docs.claude.com/en/docs/claude-code/skills
- **Example Skills**: https://github.com/anthropics/skills
- **Creating Custom Skills**: https://support.claude.com/en/articles/12512198-creating-custom-skills
