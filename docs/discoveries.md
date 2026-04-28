# Discoveries

A rolling log of non-obvious problems, solutions, and patterns discovered during amplihack-rs development. Each entry captures a single learning so future contributors avoid re-discovering the same gotchas.

**Archive policy**: When this file exceeds 100 entries, move entries older than 3 months to `discoveries-archive.md` in this directory.

## Entry Format

Each entry follows this structure:

```markdown
## Short Title (YYYY-MM-DD)

**Category**: build | ci | testing | architecture | workflow | tooling | docs

### Problem

One-paragraph description of the unexpected behavior.

### Root Cause

Why it happened — the non-obvious part worth recording.

### Solution

What fixed it and why that fix is correct.

### Key Learnings

- Bullet points a future reader can act on without reading the full entry.
```

**Guidelines**:

- One discovery per entry. If you found two things, write two entries.
- Use the date you confirmed the fix, not the date you noticed the symptom.
- Link to the relevant PR or issue when one exists.
- Keep entries factual. Skip praise, hedging, and narrative filler.

---

## Table of Contents

### April 2026

- [Docs-only rolling log chosen over Rust hook wiring](#docs-only-rolling-log-chosen-over-rust-hook-wiring-2026-04-28)

---

## Docs-only rolling log chosen over Rust hook wiring (2026-04-28)

**Category**: docs

### Problem

Issue [#435](https://github.com/rysweet/amplihack-rs/issues/435) requested a project-level `DISCOVERIES.md` mirroring the upstream `amplifier-bundle/context/DISCOVERIES.md`. Two options were proposed: (1) a lightweight `docs/discoveries.md` rolling log, or (2) wiring a Rust hook in `crates/amplihack-hooks/` to auto-capture discoveries at session boundaries.

### Root Cause

The upstream discoveries file works well as a manually-curated knowledge base. Automating capture via hooks would require changes to the Rust hook crate, a new CLI subcommand, and persistent storage plumbing — all for uncertain value since the best discoveries are written by humans reflecting on what surprised them.

### Solution

Option 1: create `docs/discoveries.md` with a clear format guide and seed entry. No Rust code changes. The file lives alongside the rest of the project documentation and is linked from `docs/index.md`.

### Key Learnings

- Start with the simplest version that delivers value. A markdown file with a format guide costs nothing to maintain and can be upgraded later.
- Hook-based automation is a separate concern. If demand emerges, a future issue can wire `amplihack discoveries add "..."` without changing this file's structure.
- The upstream `amplifier-bundle/context/DISCOVERIES.md` (2,400+ lines, Oct 2025 – Jan 2026) validates the format: problem / root cause / solution / key learnings works well at scale.
