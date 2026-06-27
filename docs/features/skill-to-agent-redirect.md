# Skill-to-Agent Redirect

**A PreToolUse guardrail that intercepts `Skill` invocations naming an agent-only target and redirects the model to agent execution instead of letting the runtime abort with `Skill not found`.**

> [Home](../index.md) > [Features](README.md) > Skill-to-Agent Redirect

## Quick Navigation

- [Skill-to-Agent Redirect API reference](../reference/skill-agent-redirect-api.md)
- [Hook specifications reference](../reference/hook-specifications.md)
- [How to troubleshoot hooks](../howto/troubleshoot-hooks.md)

---

## What This Feature Does

Some amplihack capabilities exist **only as agents** (for example `prompt-writer`), never as skills. During recipe-driven runs the model is sometimes nudged — by injected USER_PREFERENCES, the auto-intent-router reminder, or an agent system prompt — to reach a capability through the `Skill` tool. When the named target is not a registered skill, the copilot runtime's skill dispatcher logs:

```
skill(prompt-writer) Skill not found: prompt-writer
```

and the step is aborted. For `default-workflow` this silently degrades the **requirements-clarification phase**, which is supposed to run `prompt-writer` as an agent.

The Skill-to-Agent Redirect closes that gap. The `PreToolUse` hook inspects every `Skill` call **before** the runtime can fail it. When the named target:

- is **not** a known amplihack skill, **and**
- **is** a known amplihack agent,

the hook blocks the call and returns a clear, non-fatal redirect message instructing the model to invoke the capability through the agent/`Task` tool. The model self-corrects on the next turn, the requirements-clarification phase runs `prompt-writer` as an agent, and the workflow is never silently skipped or aborted.

This is a general defense: it protects against *any* agent-only name being called as a skill, not just `prompt-writer`.

---

## How It Works

```
Skill(name="prompt-writer")
        │
        ▼
┌─────────────────────────────────────────────┐
│ PreToolUse hook                              │
│  1. Launcher context injection (side effect) │
│  2. XPIA security check                      │
│  3. check_skill_redirect()  ◀── this feature │
│        is_amplihack_skill(name)? ── yes ─▶ pass through (resolve as skill)
│        is_amplihack_agent(name)? ── yes ─▶ BLOCK + redirect message
│        otherwise ──────────────────────▶ pass through
│  4. Bash-only checks (cwd, branch, ...)      │
└─────────────────────────────────────────────┘
        │
        ▼ (block)
{ "block": true, "message": "<redirect guidance>" }
```

Key properties:

- **Skill precedence.** The redirect fires only when `!is_amplihack_skill(name) && is_amplihack_agent(name)`. Names that exist as **both** a skill and an agent (for example `gherkin-expert` and `tla-plus-expert`, which ship both a `SKILL.md` and an agent file) keep resolving as skills with no behavior change.
- **Non-fatal.** The hook returns a `block` with guidance — it never panics, never aborts the run, and never deletes work. The model is told what to do instead.
- **Compile-time registry.** Agent names are baked into the binary at build time via a sorted static slice; no filesystem access happens at runtime.
- **Fail-open.** Consistent with the rest of `PreToolUse`, any parse or lookup failure passes the tool call through unchanged (`FailurePolicy::Open`).

---

## What the Model Sees

When the redirect fires, the model receives a `block` response whose `message` is static guidance plus the sanitized target name. For example, a `Skill(name="prompt-writer")` call returns:

```text
"prompt-writer" is an amplihack agent, not a skill. The Skill tool cannot
run it. Invoke it as an agent instead (use the Task/agent tool with
agent type "prompt-writer"), or reference it from a recipe step as
`agent: "amplihack:prompt-writer"`. Do not retry this as a Skill call.
```

The message intentionally echoes **only** the sanitized target name (`[A-Za-z0-9-]`), never the full tool input, so surrounding prompt content is not leaked into logs or transcripts.

---

## Behavior Matrix

| `Skill` target            | Known skill? | Known agent? | Result                          |
| ------------------------- | ------------ | ------------ | ------------------------------- |
| `prompt-writer`           | no           | yes          | **Blocked + redirect to agent** |
| `architect`               | no           | yes          | **Blocked + redirect to agent** |
| `builder`                 | no           | yes          | **Blocked + redirect to agent** |
| `default-workflow`        | yes          | no           | Pass through (runs as skill)    |
| `pdf`                     | yes          | no           | Pass through (runs as skill)    |
| `gherkin-expert`          | yes          | yes          | Pass through (skill precedence) |
| `tla-plus-expert`         | yes          | yes          | Pass through (skill precedence) |
| `guide`                   | no           | yes          | **Blocked + redirect to agent** |
| `totally-unknown-name`    | no           | no           | Pass through (no opinion)       |
| malformed input (`{}`, …) | n/a          | n/a          | Pass through (no panic)         |

---

## Operational Guarantees

| Guarantee                          | Behavior                                                                                          | Why it matters                                                        |
| ---------------------------------- | ------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------- |
| No silent skip                     | An agent-only `Skill` call is blocked with guidance instead of aborting the step.                | The requirements-clarification phase reliably runs `prompt-writer`.   |
| No false blocks                    | Overlapping names resolve as skills; unknown names pass through untouched.                        | Genuine skills and unrelated tools are never disrupted.               |
| Panic-free                         | Total, panic-free parsing of the `Skill` payload (`skill` key with `name` fallback).             | Malformed or hostile JSON cannot crash the hook (DoS resistance).     |
| Fail-open                          | Any internal failure passes the original call through.                                           | The guardrail never becomes a new failure mode.                       |
| No prompt leakage                  | The redirect echoes only the sanitized target name, never the full tool input.                   | Surrounding prompt content stays out of logs and transcripts.         |

---

## Affected Workflows

The redirect applies to **every** `PreToolUse` event, so it protects all recipes. It was introduced to fix the `default-workflow` requirements-clarification regression where `prompt-writer` was called as a skill (issue [#838](https://github.com/rysweet/amplihack-rs/issues/838)).

No configuration is required and there are no new flags or environment variables — the guardrail is always on.

---

## Related

- [Skill-to-Agent Redirect API reference](../reference/skill-agent-redirect-api.md) — `known_agents` registry and `check_skill_redirect()` developer API.
- [Hook specifications reference](../reference/hook-specifications.md) — where the `PreToolUse` hook sits in the hook table.
- [Workflow execution guardrails](workflow-execution-guardrails.md) — companion safety guardrails for recipe-driven runs.
