---
name: session-learning
version: 1.0.0
description: >-
  Record discoveries in DISCOVERIES.md for the SessionStart hook and reusable solutions
  in PATTERNS.md. Use at the end of a session or after solving a hard problem.
---

# Session Learning

## What the hooks do

The Stop hook stores session learnings in the configured memory backend. When a
prompt names a recognized agent or invokes one by slash command, the prompt hook
looks up stored memories and adds relevant results to the prompt context. The
SessionStart hook loads a nonempty `DISCOVERIES.md` from the project root under
**Recent Learnings**.

## When to record a learning

Record a learning when you:

- Solve a problem that took more than 10 minutes.
- Discover non-obvious tool behavior.
- Find a pattern that applies broadly.
- Make an architecture decision with trade-offs.

Skip an entry when:

- The issue was a trivial typo or syntax error.
- The solution is already in `DISCOVERIES.md` or `PATTERNS.md`.
- The insight is too project-specific to reuse.

## Record a discovery

Add the problem, its root cause, and the working solution to the project's
root-level `DISCOVERIES.md`.

```markdown
## [Brief Title] (YYYY-MM-DD)

### Issue

What problem or challenge was encountered?

### Root Cause

Why did this happen? What was the underlying issue?

### Solution

How was it resolved? Include code examples if relevant.

### Key Learnings

What insights were gained? What should be remembered?

### Prevention

How can this be avoided in the future?
```

- [ ] Name the specific issue.
- [ ] Identify why it happened.
- [ ] Describe a proven solution.
- [ ] State the learning in a way that applies beyond this case.
- [ ] Give an actionable prevention step.
- [ ] Stick to established facts and current needs.
- [ ] Include code examples where relevant.

## Record a pattern

Add proven solutions to recurring problems to `PATTERNS.md` wherever the project
keeps it. This file is a reference for people and for models directed to it.
Hook context loading leaves it out.

```markdown
## Pattern: [Name]

### Challenge

What problem does this pattern solve?

### Solution

How does the pattern work? Include code/examples.

### Key Points

- Main insight 1
- Main insight 2
- When to use the pattern and when to avoid it

### When to Use

Specific scenarios where this pattern applies.

### Real Impact

Where has this pattern been used successfully?

### Related Patterns

Links to similar or complementary patterns.
```

- [ ] Describe a clear, recognizable problem.
- [ ] Show that the solution has worked at least twice.
- [ ] Explain when to use the pattern and when to avoid it.
- [ ] Make the pattern general enough for reuse.
- [ ] Include clear, working code examples.
- [ ] Link to related patterns.
- [ ] Document actual use or impact.

## Create an agent

Use the `session-to-agent` skill to turn a repeatable workflow into a new agent.
