# CLAUDE.md — Amplihack agent context

This file is the canonical entry point read by Claude Code (and equivalent
agent runtimes) when they spawn inside an amplihack-managed workspace. It is
staged to `$AMPLIHACK_HOME/CLAUDE.md` by `amplihack install` so that the
verifier's framework-asset presence check finds it.

## What amplihack provides

Amplihack is a meta-framework for agentic development workflows. The bundle
ships the `amplifier-bundle/` tree containing:

- `recipes/` — declarative multi-step workflows executed by `recipe-runner-rs`
  (notably `smart-orchestrator`, `default-workflow`, `investigation-workflow`).
- `agents/`, `skills/`, `behaviors/`, `modules/` — reusable building blocks
  composed by recipes and the dev-orchestrator skill.
- `context/` — philosophy, patterns, and trust-model documents that set the
  contract for how generated code, tests, and PRs are evaluated.
- `tools/` — bundled shell/helper assets referenced by recipes.

## Where to start

For most tasks, invoking the `dev-orchestrator` skill (which routes through
`amplihack recipe run smart-orchestrator`) is the correct entry point. See
`amplifier-bundle/context/PHILOSOPHY.md` for the project's core invariants,
including the **install-completeness invariant**: `amplihack install` must
fail loudly whenever a required component cannot be staged.

## Pointer

The full framework documentation lives under `$AMPLIHACK_HOME/.claude/` and
`$AMPLIHACK_HOME/amplifier-bundle/context/` after install.
