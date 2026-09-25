---
name: agentic-workflow-first
description: Start new work as a workflow of agentic steps and tools, not code. Use when deciding how to structure a feature.
metadata:
  version: "1.0"
  author: amplihack
---

# Agentic-Workflow-First

## Purpose

Steer new work toward **deterministic workflows made of agentic steps (prompts) +
tool calls + a thin deterministic rail**, and away from imperative code. Judgment
belongs in prompts; side effects belong in tools; code is only glue.

## When I Activate

I load when you mention: designing or structuring a feature, "how should I build
this", "should this be code or a recipe", starting new work, "make this agentic",
or "reduce the code".

## The Rule (why this skill exists)

> Most core functionality should be a collection of deterministic workflows with
> agentic steps, prompts, and access to tools — NOT code. Rust is only a thin
> deterministic rail.

Default behavior is to write code. This skill forces the opposite default: prove a
step *cannot* be a prompt or a tool before you write imperative logic for it.

## The 6-Step Decision Procedure

Follow these in order for any new piece of work.

### Step 1 — Restate as goal + observable success criteria
Write the desired behavior as a single goal sentence and a numbered list of
*observable, verifiable* success criteria (what a passing run produces). If you
cannot state how to observe success, you are not ready to design.

### Step 2 — Decompose into a deterministic workflow
Break the goal into an ordered (or branching) sequence of steps. Each step has one
responsibility and a named output. This ordered set IS the workflow.

### Step 3 — Classify every step (the core test)
Tag each step as exactly one of:

- **(a) Agentic step** — an LLM/agent reasoning step driven by a prompt. Choose this
  whenever the step requires *judgment, classification, prioritization, interpretation,
  or open-ended decision*.
- **(b) Tool call** — a deterministic capability with a side effect: CLI, API, file
  IO, `git`, `gh`. Choose this for anything with an external effect or a single
  correct deterministic result.
- **(c) Thin rail** — minimal glue code: typed-record IO, guardrails, attempt caps,
  fail-closed reads. Choose this ONLY for plumbing that is neither judgment nor an
  external capability.

**Classification test — apply literally:**
- Does the step *decide something* a reasonable person could disagree on? → **(a)**.
- Does the step *change the world or read one true value* (write a file, call `gh`,
  run a command)? → **(b)**.
- Is it *pure plumbing* moving typed data between (a) and (b) with guardrails? → **(c)**.
- If tempted to write a Rust `classify()` / `orient()` / `decide()` / `prioritize()`
  function → it is judgment → it MUST be **(a)** behind a prompt.

### Step 4 — Express the workflow as a recipe
Write the workflow as a recipe YAML (repo format — see `templates.md`). Agentic steps
use `type: "agent"` with a `prompt:` block; tools use `type: "bash"`. Data flows
between steps as **typed records** (a file/JSON schema written by one step, read
fail-closed by the next) — NEVER by scraping/parsing another step's stdout prose.

### Step 5 — Write code only for the thin rail
Write imperative code ONLY for step-type (c), and only for what genuinely cannot be a
recipe/prompt/tool. For every line of imperative *judgment*, write a one-line
justification of why it is not an agentic step. If you cannot justify it, convert it.

### Step 6 — Enable + verify end-to-end
The workflow must be runnable and observable. Run it, confirm each success criterion
from Step 1 is produced, and confirm the typed-record handoffs (not stdout scraping)
carry the data.

## Anti-Patterns to Reject

See `reference.md` for the four named anti-patterns and their replacements:
1. Brittle stdout / JSON scraping of agent output → typed, owner-only (`0o600`),
   freshness-checked records read **fail-CLOSED**.
2. Imperative heuristics standing in for judgment (`classify_signal`, `orient`,
   `decide`) → an agentic step behind a prompt.
3. Wall-clock timeouts that kill working agentic steps → idle/liveness detection.
4. Passing large payloads via argv/env (E2BIG) → route via stdin or a file the tool
   reads.

## Templates & Example

- `templates.md` — copy-ready recipe, prompt, typed-record, and fail-closed-reader
  skeletons matching the repo's real shapes.
- `worked-example.md` — one behavior (classify + route a signal) built as recipe +
  prompt + typed record + ~10-line rail, contrasted with a naive hardcoded function.

## §7 — Honest note on reflexive irony

Some shipped recipes in this repo still hand data between steps by having a later step
parse an earlier step's stdout (e.g. `parse_json` over agent output). That works, and
this skill does not claim those recipes are broken. The **recipe step is the agentic
unit** either way. What this skill teaches is the *improved handoff*: a step writes a
**typed record** (owner-only, freshness-checked) and the next step reads it fail-closed.
Model new work on the typed-record handoff; treat stdout-scraping as legacy, not as the
target pattern.
